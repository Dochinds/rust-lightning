// Imports that need to be added manually
use bitcoin::bech32::u5;
use bitcoin::blockdata::script::Script;
use bitcoin::secp256k1::{PublicKey, Scalar, SecretKey};
use bitcoin::secp256k1::ecdh::SharedSecret;
use bitcoin::secp256k1::ecdsa::RecoverableSignature;

use lightning::chain::keysinterface::{Recipient, KeyMaterial, KeysInterface};
use lightning::ln::msgs::{self, DecodeError, OnionMessageHandler};
use lightning::ln::script::ShutdownScript;
use lightning::util::enforcing_trait_impls::EnforcingSigner;
use lightning::util::logger::Logger;
use lightning::util::ser::{Readable, Writer};
use lightning::onion_message::OnionMessenger;

use utils::test_logger;

use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};

#[inline]
/// Actual fuzz test, method signature and name are fixed
pub fn do_test<L: Logger>(data: &[u8], logger: &L) {
	if let Ok(msg) = <msgs::OnionMessage as Readable>::read(&mut Cursor::new(data)) {
		let mut secret_bytes = [0; 32];
		secret_bytes[31] = 2;
		let secret = SecretKey::from_slice(&secret_bytes).unwrap();
		let keys_manager = KeyProvider {
			node_secret: secret,
			counter: AtomicU64::new(0),
		};
		let onion_messenger = OnionMessenger::new(&keys_manager, logger);
		let mut pk = [2; 33]; pk[1] = 0xff;
		let peer_node_id_not_used = PublicKey::from_slice(&pk).unwrap();
		onion_messenger.handle_onion_message(&peer_node_id_not_used, &msg);
	}
}

/// Method that needs to be added manually, {name}_test
pub fn onion_message_test<Out: test_logger::Output>(data: &[u8], out: Out) {
	let logger = test_logger::TestLogger::new("".to_owned(), out);
	do_test(data, &logger);
}

/// Method that needs to be added manually, {name}_run
#[no_mangle]
pub extern "C" fn onion_message_run(data: *const u8, datalen: usize) {
	let logger = test_logger::TestLogger::new("".to_owned(), test_logger::DevNull {});
	do_test(unsafe { std::slice::from_raw_parts(data, datalen) }, &logger);
}

pub struct VecWriter(pub Vec<u8>);
impl Writer for VecWriter {
	fn write_all(&mut self, buf: &[u8]) -> Result<(), ::std::io::Error> {
		self.0.extend_from_slice(buf);
		Ok(())
	}
}
struct KeyProvider {
	node_secret: SecretKey,
	counter: AtomicU64,
}
impl KeysInterface for KeyProvider {
	type Signer = EnforcingSigner;

	fn get_node_secret(&self, _recipient: Recipient) -> Result<SecretKey, ()> {
		Ok(self.node_secret.clone())
	}

	fn ecdh(&self, recipient: Recipient, other_key: &PublicKey, tweak: Option<&Scalar>) -> Result<SharedSecret, ()> {
		let mut node_secret = self.get_node_secret(recipient)?;
		if let Some(tweak) = tweak {
			node_secret = node_secret.mul_tweak(tweak).map_err(|_| ())?;
		}
		Ok(SharedSecret::new(other_key, &node_secret))
	}

	fn get_inbound_payment_key_material(&self) -> KeyMaterial { unreachable!() }

	fn get_destination_script(&self) -> Script { unreachable!() }

	fn get_shutdown_scriptpubkey(&self) -> ShutdownScript { unreachable!() }

	fn get_channel_signer(&self, _inbound: bool, _channel_value_satoshis: u64) -> EnforcingSigner {
		unreachable!()
	}

	fn get_secure_random_bytes(&self) -> [u8; 32] {
		let ctr = self.counter.fetch_add(1, Ordering::Relaxed);
		[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
		(ctr >> 8*7) as u8, (ctr >> 8*6) as u8, (ctr >> 8*5) as u8, (ctr >> 8*4) as u8, (ctr >> 8*3) as u8, (ctr >> 8*2) as u8, (ctr >> 8*1) as u8, 14, (ctr >> 8*0) as u8]
	}

	fn read_chan_signer(&self, _data: &[u8]) -> Result<EnforcingSigner, DecodeError> { unreachable!() }

	fn sign_invoice(&self, _hrp_bytes: &[u8], _invoice_data: &[u5], _recipient: Recipient) -> Result<RecoverableSignature, ()> {
		unreachable!()
	}
}

#[cfg(test)]
mod tests {
	use lightning::util::logger::{Logger, Record};
	use std::collections::HashMap;
	use std::sync::Mutex;

	struct TrackingLogger {
		/// (module, message) -> count
		pub lines: Mutex<HashMap<(String, String), usize>>,
	}
	impl Logger for TrackingLogger {
		fn log(&self, record: &Record) {
			*self.lines.lock().unwrap().entry((record.module_path.to_string(), format!("{}", record.args))).or_insert(0) += 1;
			println!("{:<5} [{} : {}, {}] {}", record.level.to_string(), record.module_path, record.file, record.line, record.args);
		}
	}

	struct MessengerNode {
		keys_manager: Arc<test_utils::TestKeysInterface>,
		messenger: OnionMessenger<EnforcingSigner, Arc<test_utils::TestKeysInterface>, Arc<test_utils::TestLogger>>,
		logger: Arc<test_utils::TestLogger>,
	}

	impl MessengerNode {
		fn get_node_pk(&self) -> PublicKey {
			let secp_ctx = Secp256k1::new();
			PublicKey::from_secret_key(&secp_ctx, &self.keys_manager.get_node_secret(Recipient::Node).unwrap())
		}
	}
	fn create_nodes(num_messengers: u8) -> Vec<MessengerNode> {
		let mut nodes = Vec::new();
		for i in 0..num_messengers {
			let logger = Arc::new(test_utils::TestLogger::with_id(format!("node {}", i)));
			let seed = [i as u8; 32];
			let keys_manager = Arc::new(test_utils::TestKeysInterface::new(&seed, Network::Testnet));
			nodes.push(MessengerNode {
				keys_manager: keys_manager.clone(),
				messenger: OnionMessenger::new(keys_manager, logger.clone()),
				logger,
			});
		}
		for idx in 0..num_messengers - 1 {
			let i = idx as usize;
			let mut features = InitFeatures::empty();
			features.set_onion_messages_optional();
			let init_msg = msgs::Init { features, remote_network_address: None };
			nodes[i].messenger.peer_connected(&nodes[i + 1].get_node_pk(), &init_msg.clone()).unwrap();
			nodes[i + 1].messenger.peer_connected(&nodes[i].get_node_pk(), &init_msg.clone()).unwrap();
		}
		nodes
	}
	#[test]
	fn test_no_onion_message_breakage() {
		let one_hop_om = "020000000000000000000000000000000000000000000000000000000000000e01055600020000000000000000000000000000000000000000000000000000000000000e01120410950000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000009300000000000000000000000000000000000000000000000000000000000000";
		let logger = TrackingLogger { lines: Mutex::new(HashMap::new()) };
		super::do_test(&::hex::decode(one_hop_om).unwrap(), &logger);
		{
			let log_entries = logger.lines.lock().unwrap();
			assert_eq!(log_entries.get(&("lightning::onion_message::messenger".to_string(), "Received an onion message with path_id: None and no reply_path".to_string())), Some(&1));
		}

		let two_unblinded_hops_om = "020000000000000000000000000000000000000000000000000000000000000e01055600020000000000000000000000000000000000000000000000000000000000000e0135043304210200000000000000000000000000000000000000000000000000000000000000039500000000000000000000000000000058000000000000000000000000000000000000000000000000000000000000001204105e00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b300000000000000000000000000000000000000000000000000000000000000";
		let logger = TrackingLogger { lines: Mutex::new(HashMap::new()) };
		super::do_test(&::hex::decode(two_unblinded_hops_om).unwrap(), &logger);
		{
			let log_entries = logger.lines.lock().unwrap();
			assert_eq!(log_entries.get(&("lightning::onion_message::messenger".to_string(), "Forwarding an onion message to peer 020000000000000000000000000000000000000000000000000000000000000003".to_string())), Some(&1));
		}

		let two_unblinded_two_blinded_om = "020000000000000000000000000000000000000000000000000000000000000e01055600020000000000000000000000000000000000000000000000000000000000000e01350433042102000000000000000000000000000000000000000000000000000000000000000395000000000000000000000000000000530000000000000000000000000000000000000000000000000000000000000058045604210200000000000000000000000000000000000000000000000000000000000000040821020000000000000000000000000000000000000000000000000000000000000e015e0000000000000000000000000000006b0000000000000000000000000000000000000000000000000000000000000035043304210200000000000000000000000000000000000000000000000000000000000000054b000000000000000000000000000000e800000000000000000000000000000000000000000000000000000000000000120410ee00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b300000000000000000000000000000000000000000000000000000000000000";
		let logger = TrackingLogger { lines: Mutex::new(HashMap::new()) };
		super::do_test(&::hex::decode(two_unblinded_two_blinded_om).unwrap(), &logger);
		{
			let log_entries = logger.lines.lock().unwrap();
			assert_eq!(log_entries.get(&("lightning::onion_message::messenger".to_string(), "Forwarding an onion message to peer 020000000000000000000000000000000000000000000000000000000000000003".to_string())), Some(&1));
		}

		let three_blinded_om = "020000000000000000000000000000000000000000000000000000000000000e01055600020000000000000000000000000000000000000000000000000000000000000e013504330421020000000000000000000000000000000000000000000000000000000000000003950000000000000000000000000000007f0000000000000000000000000000000000000000000000000000000000000035043304210200000000000000000000000000000000000000000000000000000000000000045e0000000000000000000000000000004c000000000000000000000000000000000000000000000000000000000000001204104a0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b300000000000000000000000000000000000000000000000000000000000000";
		let logger = TrackingLogger { lines: Mutex::new(HashMap::new()) };
		super::do_test(&::hex::decode(three_blinded_om).unwrap(), &logger);
		{
			let log_entries = logger.lines.lock().unwrap();
			assert_eq!(log_entries.get(&("lightning::onion_message::messenger".to_string(), "Forwarding an onion message to peer 020000000000000000000000000000000000000000000000000000000000000003".to_string())), Some(&1));
		}
	}
}
