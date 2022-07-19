// This file is Copyright its original authors, visible in version control
// history.
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Creating blinded routes and related utilities live here.

use bitcoin::secp256k1::{self, PublicKey, Secp256k1, SecretKey};

use chain::keysinterface::{KeysInterface, Sign};
use super::utils;
use util::chacha20poly1305rfc::ChaChaPolyWriteAdapter;
use util::ser::{VecWriter, Writeable, Writer};

use core::iter::FromIterator;
use io;
use prelude::*;

/// Onion messages can be sent and received to blinded routes, which serve to hide the identity of
/// the recipient.
pub struct BlindedRoute {
	/// To send to a blinded route, the sender first finds a route to the unblinded
	/// `introduction_node_id`, which can unblind its [`encrypted_payload`] to find out the onion
	/// message's next hop and forward it along.
	///
	/// [`encrypted_payload`]: BlindedHop::encrypted_payload
	introduction_node_id: PublicKey,
	/// Used by the introduction node to decrypt its [`encrypted_payload`] to forward the onion
	/// message.
	///
	/// [`encrypted_payload`]: BlindedHop::encrypted_payload
	blinding_point: PublicKey,
	/// The hops composing the blinded route.
	blinded_hops: Vec<BlindedHop>,
}

/// Used to construct the blinded hops portion of a blinded route. These hops cannot be identified
/// by outside observers and thus can be used to hide the identity of the recipient.
pub struct BlindedHop {
	/// The blinded node id of this hop in a blinded route.
	blinded_node_id: PublicKey,
	/// The encrypted payload intended for this hop in a blinded route.
	// The node sending to this blinded route will later encode this payload into the onion packet for
	// this hop.
	encrypted_payload: Vec<u8>,
}

impl BlindedRoute {
	/// Create a blinded route to be forwarded along `node_pks`. The last node pubkey in `node_pks`
	/// will be the destination node.
	///
	/// Errors if less than two hops are provided or if `node_pk`(s) are invalid.
	//  TODO: make all payloads the same size with padding + add dummy hops
	pub fn new<Signer: Sign, K: KeysInterface, T: secp256k1::Signing + secp256k1::Verification>
		(node_pks: Vec<PublicKey>, keys_manager: &K, secp_ctx: &Secp256k1<T>) -> Result<Self, ()>
	{
		if node_pks.len() < 2 { return Err(()) }
		let blinding_secret_bytes = keys_manager.get_secure_random_bytes();
		let blinding_secret = SecretKey::from_slice(&blinding_secret_bytes[..]).expect("RNG is busted");
		let mut route_keys = construct_blinded_route_keys(secp_ctx, &node_pks, &blinding_secret).map_err(|_| ())?;
		let (final_enc_payload_key, final_blinded_node_id) = route_keys.pop().unwrap();
		let mut blinded_hops = Vec::with_capacity(node_pks.len());

		let mut pks = VecDeque::from_iter(node_pks);
		let introduction_node_id = pks.pop_front().unwrap();
		for (pk, (encrypted_payload_key, blinded_node_id)) in pks.into_iter().zip(route_keys) {
			let payload = ForwardTlvs {
				next_node_id: pk,
				next_blinding_override: None,
			};
			blinded_hops.push(BlindedHop {
				blinded_node_id,
				encrypted_payload: encrypt_payload(payload, encrypted_payload_key),
			});
		}

		// Add the recipient final payload.
		let payload = ReceiveTlvs { path_id: None };
		blinded_hops.push(BlindedHop {
			blinded_node_id: final_blinded_node_id,
			encrypted_payload: encrypt_payload(payload, final_enc_payload_key),
		});

		Ok(BlindedRoute {
			introduction_node_id,
			blinding_point: PublicKey::from_secret_key(secp_ctx, &blinding_secret),
			blinded_hops,
		})
	}
}

/// Construct keys for creating a blinded route along the given `unblinded_path`.
///
/// Returns: `Vec<(encrypted_payload_key, blinded_node_id)>`
/// where the former is for encrypting [`BlindedHop::encrypted_payload`] and the latter for
/// [`BlindedHop::blinded_node_id`].
fn construct_blinded_route_keys<T: secp256k1::Signing + secp256k1::Verification>(
	secp_ctx: &Secp256k1<T>, unblinded_path: &Vec<PublicKey>, session_priv: &SecretKey
) -> Result<Vec<([u8; 32], PublicKey)>, secp256k1::Error> {
	let mut route_keys = Vec::with_capacity(unblinded_path.len());

	utils::construct_keys_callback(secp_ctx, unblinded_path, session_priv, |blinded_hop_pubkey, _, _, encrypted_payload_ss| {
		route_keys.push((encrypted_payload_ss, blinded_hop_pubkey));
	})?;

	Ok(route_keys)
}

/// Encrypt TLV payload to be used as a [`BlindedHop::encrypted_payload`].
fn encrypt_payload<P: Writeable>(payload: P, encrypted_tlvs_ss: [u8; 32]) -> Vec<u8> {
	let mut writer = VecWriter(Vec::new());
	let write_adapter = ChaChaPolyWriteAdapter::new(encrypted_tlvs_ss, &payload);
	write_adapter.write(&mut writer).expect("In-memory writes cannot fail");
	writer.0
}

/// TLVs to encode in an intermediate onion message packet's hop data. When provided in a blinded
/// route, they are encoded into [`BlindedHop::encrypted_payload`].
pub(crate) struct ForwardTlvs {
	/// The node id of the next hop in the onion message's path.
	next_node_id: PublicKey,
	/// Senders to a blinded route use this value to concatenate the route they find to the
	/// introduction node with the blinded route.
	next_blinding_override: Option<PublicKey>,
}

/// Similar to [`ForwardTlvs`], but these TLVs are for the final node.
pub(crate) struct ReceiveTlvs {
	/// If `path_id` is `Some`, it is used to identify the blinded route that this onion message is
	/// sending to. This is useful for receivers to check that said blinded route is being used in
	/// the right context.
	path_id: Option<[u8; 32]>,
}

impl Writeable for ForwardTlvs {
	fn write<W: Writer>(&self, writer: &mut W) -> Result<(), io::Error> {
		// TODO: write padding
		encode_tlv_stream!(writer, {
			(4, self.next_node_id, required),
			(8, self.next_blinding_override, option)
		});
		Ok(())
	}
}

impl Writeable for ReceiveTlvs {
	fn write<W: Writer>(&self, writer: &mut W) -> Result<(), io::Error> {
		// TODO: write padding
		encode_tlv_stream!(writer, {
			(6, self.path_id, option),
		});
		Ok(())
	}
}
