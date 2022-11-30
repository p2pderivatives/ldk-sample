use std::sync::Arc;

use dlc_manager::{
	channel::{signed_channel::SignedChannelState, Channel},
	contract::{
		numerical_descriptor::NumericalDescriptor, signed_contract::SignedContract, Contract,
		ContractDescriptor,
	},
	payout_curve::{
		HyperbolaPayoutCurvePiece, PayoutFunction, PayoutFunctionPiece, PayoutPoint,
		PolynomialPayoutCurvePiece, RoundingInterval, RoundingIntervals,
	},
	sub_channel_manager::{SubChannel, SubChannelState},
	Storage,
};
use dlc_sled_storage_provider::SledStorageProvider;
use dlc_trie::OracleNumericInfo;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct LastPrice {
	pub ltp: f64,
}

pub(crate) fn build_cfd_contract(
	max_value: u64, principal_sats: u64, total_collateral_sats: u64, btcusd_rate: u64,
) -> Result<ContractDescriptor, dlc_manager::error::Error> {
	assert!(principal_sats < total_collateral_sats);
	let satusd_rate = btcusd_rate as f64 / 100000000.0;
	let principal_usd = (principal_sats as f64) * satusd_rate;
	let min_limit = (principal_usd * 100000000.0 / (total_collateral_sats as f64)).ceil() as u64;
	let collar_point = PayoutPoint {
		event_outcome: min_limit,
		outcome_payout: total_collateral_sats,
		extra_precision: 0,
	};
	let collar =
		PayoutFunctionPiece::PolynomialPayoutCurvePiece(PolynomialPayoutCurvePiece::new(vec![
			PayoutPoint {
				event_outcome: 0,
				outcome_payout: total_collateral_sats,
				extra_precision: 0,
			},
			collar_point.clone(),
		])?);
	let right_payout = (principal_usd * 100000000.0 / max_value as f64).round() as u64;
	let right_end_point =
		PayoutPoint { event_outcome: max_value, outcome_payout: right_payout, extra_precision: 0 };
	let hyperbola = PayoutFunctionPiece::HyperbolaPayoutCurvePiece(HyperbolaPayoutCurvePiece::new(
		collar_point,
		right_end_point,
		true,
		0.0,
		0.0,
		btcusd_rate as f64,
		0.0,
		0.0,
		principal_sats as f64,
	)?);
	let rounding_intervals = RoundingIntervals {
		intervals: vec![RoundingInterval { begin_interval: 0, rounding_mod: 3 }],
	};
	let payout_function = PayoutFunction::new(vec![collar, hyperbola])?;
	let descriptor = NumericalDescriptor {
		payout_function,
		rounding_intervals,
		difference_params: None,
		oracle_numeric_infos: OracleNumericInfo { base: 2, nb_digits: vec![20] },
	};
	Ok(ContractDescriptor::Numerical(descriptor))
}

pub(crate) fn get_subchannel_contract(
	sub_channel: &SubChannel, storage: &Arc<SledStorageProvider>,
) -> Option<SignedContract> {
	match sub_channel.state {
		SubChannelState::Signed(_) => {
			let channel = storage.get_channel(&sub_channel.channel_id).unwrap().unwrap();
			let contract_id = match channel {
				Channel::Signed(s) => {
					if let SignedChannelState::Established { .. } = &s.state {
						s.get_contract_id().unwrap()
					} else {
						return None;
					}
				}
				_ => unreachable!(),
			};
			let contract = storage.get_contract(&contract_id).unwrap().unwrap();
			match contract {
				Contract::Confirmed(s) => Some(s),
				Contract::Signed(_) => None,
				c => panic!("{:?}", c),
			}
		}
		_ => None,
	}
}

pub(crate) fn get_contract_payout(
	contract: &SignedContract, last_price: &LastPrice,
) -> dlc::Payout {
	let current_rate = last_price.ltp.round() as u64;
	let decomposed = dlc_trie::digit_decomposition::decompose_value(current_rate as usize, 2, 20);
	let payout_index = match &contract.accepted_contract.adaptor_infos[0] {
		dlc_manager::contract::AdaptorInfo::Numerical(n) => {
			n.look_up(&vec![(0, decomposed)])
				.expect("to have found corresponding index info")
				.0
				.cet_index
		}
		dlc_manager::contract::AdaptorInfo::Enum => unreachable!(),
		dlc_manager::contract::AdaptorInfo::NumericalWithDifference(_) => {
			unreachable!()
		}
	};
	let payout = &contract.accepted_contract.offered_contract.contract_info[0]
		.get_payouts(contract.accepted_contract.offered_contract.total_collateral)
		.unwrap()[payout_index];
	payout.clone()
}

#[cfg(test)]
mod tests {
	use dlc_manager::payout_curve::Evaluable;

	use super::*;

	#[test]
	fn contract_curve_test() {
		let contract_descriptor = build_cfd_contract(1048575, 45000, 90000, 17000).unwrap();
		let rounding_intervals = RoundingIntervals {
			intervals: vec![RoundingInterval { begin_interval: 0, rounding_mod: 4 }],
		};

		if let ContractDescriptor::Numerical(desc) = contract_descriptor {
			let curve = &desc.payout_function.payout_function_pieces[1];
			let hyperbola = match curve {
				PayoutFunctionPiece::PolynomialPayoutCurvePiece(_) => panic!("Should be hyperbola"),
				PayoutFunctionPiece::HyperbolaPayoutCurvePiece(h) => h,
			};
			// Should give same payout if value doesn't change
			assert_eq!(hyperbola.get_rounded_payout(17000, &rounding_intervals).unwrap(), 45000);
		} else {
			panic!();
		}
	}
}
