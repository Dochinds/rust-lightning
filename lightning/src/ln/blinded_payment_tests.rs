// This file is Copyright its original authors, visible in version control
// history.
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use bitcoin::secp256k1::Secp256k1;
use crate::blinded_path::BlindedPath;
use crate::blinded_path::payment::{BlindedPaymentTlvs, PaymentConstraints, PaymentRelay};
use crate::events::MessageSendEventsProvider;
use crate::ln::channelmanager::{PaymentId, RecipientOnionFields};
use crate::ln::features::BlindedHopFeatures;
use crate::ln::functional_test_utils::*;
use crate::ln::msgs::ChannelMessageHandler;
use crate::ln::outbound_payment::Retry;
use crate::prelude::*;
use crate::routing::router::{PaymentParameters, RouteParameters};

#[test]
fn simple_blinded_payment() {
	let chanmon_cfgs = create_chanmon_cfgs(4);
	let node_cfgs = create_node_cfgs(4, &chanmon_cfgs);
	let mut cfg = test_default_channel_config();
	// Test the fee_proportional_millionths specified in the blinded path's payment constraints.
	cfg.channel_config.forwarding_fee_proportional_millionths = 100;
	let node_chanmgrs = create_node_chanmgrs(4, &node_cfgs, &[None, None, Some(cfg), None]);
	let nodes = create_network(4, &node_cfgs, &node_chanmgrs);
	create_announced_chan_between_nodes_with_value(&nodes, 0, 1, 1_000_000, 0);
	create_announced_chan_between_nodes_with_value(&nodes, 1, 2, 1_000_000, 0);
	let chan_upd = create_announced_chan_between_nodes_with_value(&nodes, 2, 3, 1_000_000, 0).0.contents;

	let amt_msat = 5000;
	let (payment_preimage, payment_hash, payment_secret) = get_payment_preimage_hash(&nodes[3], Some(amt_msat), None);
	let path = vec![(nodes[2].node.get_our_node_id(), BlindedPaymentTlvs::Forward {
		short_channel_id: chan_upd.short_channel_id,
		payment_relay: PaymentRelay {
			cltv_expiry_delta: chan_upd.cltv_expiry_delta,
			fee_proportional_millionths: chan_upd.fee_proportional_millionths,
			fee_base_msat: chan_upd.fee_base_msat,
		},
		payment_constraints: PaymentConstraints {
			max_cltv_expiry: u32::max_value(),
			htlc_minimum_msat: chan_upd.htlc_minimum_msat,
		},
		features: BlindedHopFeatures::empty(),
	}), (nodes[3].node.get_our_node_id(), BlindedPaymentTlvs::Receive {
		payment_secret,
		payment_constraints: PaymentConstraints {
			max_cltv_expiry: u32::max_value(),
			htlc_minimum_msat: chan_upd.htlc_minimum_msat,
		},
		features: BlindedHopFeatures::empty(),
	})];
	let mut secp_ctx = Secp256k1::new();
	let blinded_path = BlindedPath::new_for_payment(&path[..], &chanmon_cfgs[3].keys_manager, &secp_ctx).unwrap();

	let route_params = RouteParameters {
		payment_params: PaymentParameters::blinded(vec![blinded_path]),
		final_value_msat: amt_msat
	};
	nodes[0].node.send_payment(payment_hash, RecipientOnionFields::spontaneous_empty(), PaymentId(payment_hash.0), route_params, Retry::Attempts(0)).unwrap();
	check_added_monitors(&nodes[0], 1);
	pass_along_route(&nodes[0], &[&[&nodes[1], &nodes[2], &nodes[3]]], amt_msat, payment_hash, payment_secret);
	claim_payment(&nodes[0], &[&nodes[1], &nodes[2], &nodes[3]], payment_preimage);
}

#[test]
fn blinded_mpp() {
	// TODO: start here
}

#[test]
fn blinded_intercept_payment() {
	let chanmon_cfgs = create_chanmon_cfgs(3);
	let node_cfgs = create_node_cfgs(3, &chanmon_cfgs);
	let mut intercept_forwards_config = test_default_channel_config();
	intercept_forwards_config.accept_intercept_htlcs = true;
	let node_chanmgrs = create_node_chanmgrs(3, &node_cfgs, &[None, Some(intercept_forwards_config), None]);
	let nodes = create_network(3, &node_cfgs, &node_chanmgrs);
	create_announced_chan_between_nodes_with_value(&nodes, 0, 1, 1_000_000, 0);
	let chan = create_announced_chan_between_nodes_with_value(&nodes, 1, 2, 1_000_000, 0);
	let (channel_id, chan_upd) = (chan.2, chan.0.contents);

	let amt_msat = 5000;
	let (payment_preimage, payment_hash, payment_secret) = get_payment_preimage_hash(&nodes[2], Some(amt_msat), None);
	let intercept_scid = nodes[1].node.get_intercept_scid();
	let path = vec![(nodes[1].node.get_our_node_id(), BlindedPaymentTlvs::Forward {
		short_channel_id: intercept_scid,
		payment_relay: PaymentRelay {
			cltv_expiry_delta: chan_upd.cltv_expiry_delta,
			fee_proportional_millionths: chan_upd.fee_proportional_millionths,
			fee_base_msat: chan_upd.fee_base_msat,
		},
		payment_constraints: PaymentConstraints {
			max_cltv_expiry: u32::max_value(),
			htlc_minimum_msat: chan_upd.htlc_minimum_msat,
		},
		features: BlindedHopFeatures::empty(),
	}), (nodes[2].node.get_our_node_id(), BlindedPaymentTlvs::Receive {
		payment_secret,
		payment_constraints: PaymentConstraints {
			max_cltv_expiry: u32::max_value(),
			htlc_minimum_msat: chan_upd.htlc_minimum_msat,
		},
		features: BlindedHopFeatures::empty(),
	})];
	let mut secp_ctx = Secp256k1::new();
	let blinded_path = BlindedPath::new_for_payment(&path[..], &chanmon_cfgs[2].keys_manager, &secp_ctx).unwrap();

	let route_params = RouteParameters {
		payment_params: PaymentParameters::blinded(vec![blinded_path]),
		final_value_msat: amt_msat
	};
	nodes[0].node.send_payment(payment_hash, RecipientOnionFields::spontaneous_empty(),
		PaymentId(payment_hash.0), route_params, Retry::Attempts(0)).unwrap();
	check_added_monitors(&nodes[0], 1);
	let payment_event = {
		let mut events = nodes[0].node.get_and_clear_pending_msg_events();
		assert_eq!(events.len(), 1);
		SendEvent::from_event(events.remove(0))
	};
	nodes[1].node.handle_update_add_htlc(&nodes[0].node.get_our_node_id(), &payment_event.msgs[0]);
	commitment_signed_dance!(nodes[1], nodes[0], &payment_event.commitment_msg, false, true);

	let events = nodes[1].node.get_and_clear_pending_events();
	assert_eq!(events.len(), 1);
	let (intercept_id, expected_outbound_amount_msat) = match events[0] {
		crate::events::Event::HTLCIntercepted {
			intercept_id, expected_outbound_amount_msat, payment_hash: pmt_hash,
			requested_next_hop_scid: short_channel_id, ..
		} => {
			assert_eq!(pmt_hash, payment_hash);
			assert_eq!(short_channel_id, intercept_scid);
			(intercept_id, expected_outbound_amount_msat)
		},
		_ => panic!()
	};

	nodes[1].node.forward_intercepted_htlc(intercept_id, &channel_id, nodes[2].node.get_our_node_id(),
		expected_outbound_amount_msat).unwrap();
	expect_pending_htlcs_forwardable!(nodes[1]);

	let payment_event = {
		{
			let mut added_monitors = nodes[1].chain_monitor.added_monitors.lock().unwrap();
			assert_eq!(added_monitors.len(), 1);
			added_monitors.clear();
		}
		let mut events = nodes[1].node.get_and_clear_pending_msg_events();
		assert_eq!(events.len(), 1);
		SendEvent::from_event(events.remove(0))
	};
	nodes[2].node.handle_update_add_htlc(&nodes[1].node.get_our_node_id(), &payment_event.msgs[0]);
	commitment_signed_dance!(nodes[2], nodes[1], &payment_event.commitment_msg, false, true);
	expect_pending_htlcs_forwardable!(nodes[2]);

	expect_payment_claimable!(&nodes[2], payment_hash, payment_secret, amt_msat, None,
		nodes[2].node.get_our_node_id());
	do_claim_payment_along_route(&nodes[0], &vec!(&vec!(&nodes[1], &nodes[2])[..]), false, payment_preimage);
	expect_payment_sent(&nodes[0], payment_preimage, Some(Some(1000)), true);
}

#[test]
fn one_hop_blinded_path() {
	do_one_hop_blinded_path(true);
	do_one_hop_blinded_path(false);
}

fn do_one_hop_blinded_path(success: bool) {
	let chanmon_cfgs = create_chanmon_cfgs(2);
	let node_cfgs = create_node_cfgs(2, &chanmon_cfgs);
	let node_chanmgrs = create_node_chanmgrs(2, &node_cfgs, &[None, None]);
	let nodes = create_network(2, &node_cfgs, &node_chanmgrs);
	let chan_upd = create_announced_chan_between_nodes_with_value(&nodes, 0, 1, 1_000_000, 0).0.contents;

	let amt_msat = 5000;
	let (payment_preimage, payment_hash, payment_secret) = get_payment_preimage_hash(&nodes[1], Some(amt_msat), None);
	let path = vec![(nodes[1].node.get_our_node_id(), BlindedPaymentTlvs::Receive {
		payment_secret,
		payment_constraints: PaymentConstraints {
			max_cltv_expiry: u32::max_value(),
			htlc_minimum_msat: chan_upd.htlc_minimum_msat,
		},
		features: BlindedHopFeatures::empty(),
	})];
	let mut secp_ctx = Secp256k1::new();
	let blinded_path = BlindedPath::new_for_payment(&path[..], &chanmon_cfgs[1].keys_manager, &secp_ctx).unwrap();

	let route_params = RouteParameters {
		payment_params: PaymentParameters::blinded(vec![blinded_path]),
		final_value_msat: amt_msat
	};
	nodes[0].node.send_payment(payment_hash, RecipientOnionFields::spontaneous_empty(),
		PaymentId(payment_hash.0), route_params, Retry::Attempts(0)).unwrap();
	check_added_monitors(&nodes[0], 1);
	pass_along_route(&nodes[0], &[&[&nodes[1]]], amt_msat, payment_hash, payment_secret);
	if success {
		claim_payment(&nodes[0], &[&nodes[1]], payment_preimage);
	} else {
		fail_payment(&nodes[0], &[&nodes[1]], payment_hash);
	}
}