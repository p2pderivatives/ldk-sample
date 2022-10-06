// This code is mainly copied and adapted from the LdkSample (https://github.com/lightningdevkit/ldk-sample)
use super::DlcManager;
use super::DlcMessageHandler;
use super::PeerManager;
use super::SubChannelManager;
use crate::hex_utils;
use dlc_manager::channel::signed_channel::SignedChannelState;
use dlc_manager::channel::signed_channel::SignedChannelStateType;
use dlc_manager::contract::contract_input::ContractInput;
use dlc_manager::contract::{ClosedContract, Contract};
use dlc_manager::Oracle;
use dlc_manager::Storage;
use dlc_messages::sub_channel::SubChannelMessage;
use dlc_messages::Message as DlcMessage;
use hex_utils::{hex_str, to_slice};
use p2pd_oracle_client::P2PDOracleClient;
use std::fs;
use std::io;
use std::io::{BufRead, Write};
use std::str::SplitWhitespace;
use std::sync::Arc;

macro_rules! read_id_or_continue {
	($words: ident, $err_cmd: expr, $err_arg: expr) => {
		match read_id(&mut $words, $err_cmd, $err_arg) {
			Ok(res) => res,
			Err(()) => continue,
		}
	};
}

pub(crate) async fn poll_for_user_input(
	peer_manager: Arc<PeerManager>, dlc_message_handler: Arc<DlcMessageHandler>,
	dlc_manager: Arc<DlcManager>, sub_channel_manager: Arc<SubChannelManager>,
	oracle: Arc<P2PDOracleClient>, offers_path: &str,
) {
	println!("To view available commands: \"help\".");
	let manager_clone = dlc_manager.clone();
	tokio::task::spawn_blocking(move || {
		manager_clone.periodic_check().expect("Error doing periodic check.");
	});
	let stdin = io::stdin();
	print!("> ");
	io::stdout().flush().unwrap(); // Without flushing, the `>` doesn't print
	for line in stdin.lock().lines() {
		process_incoming_messages(
			&peer_manager,
			&dlc_manager,
			&sub_channel_manager,
			&dlc_message_handler,
		);
		let line = line.unwrap();
		if line.is_empty() {
			break;
		}
		let mut words = line.split_whitespace();
		if let Some(word) = words.next() {
			match word {
				"help" => help(),
				o @ "offercontract" | o @ "offerchannel" => {
					let (peer_pubkey_and_ip_addr, contract_path) = match (
						words.next(),
						words.next(),
					) {
						(Some(pp), Some(cp)) => (pp, cp),
						_ => {
							println!("ERROR: offercontract requires peer connection info and contract path: `offercontract pubkey@host:port contract_path`");
							print!("> ");
							io::stdout().flush().unwrap();
							continue;
						}
					};
					let (pubkey, peer_addr) =
						match crate::cli::parse_peer_info(peer_pubkey_and_ip_addr.to_string()) {
							Ok(info) => info,
							Err(e) => {
								println!("{:?}", e.into_inner().unwrap());
								print!("> ");
								io::stdout().flush().unwrap();
								continue;
							}
						};
					if crate::cli::connect_peer_if_necessary(
						pubkey,
						peer_addr,
						peer_manager.clone(),
					)
					.await
					.is_ok()
					{
						println!("SUCCESS: connected to peer {}", pubkey);
					}
					let contract_input_str = fs::read_to_string(&contract_path)
						.expect("Error reading contract input file.");
					let contract_input: ContractInput = serde_json::from_str(&contract_input_str)
						.expect("Error deserializing contract input.");
					let is_sub_channel = o == "offersubchannel";
					if is_sub_channel {
						let channel_id = read_id_or_continue!(words, o, "channel id");
						let contract_input_clone = contract_input.clone();
						let oracle_clone = oracle.clone();
						let manager_clone = sub_channel_manager.clone();
						let sub_channel_offer = tokio::task::spawn_blocking(move || {
							let announcement = oracle_clone
								.get_announcement(
									&contract_input_clone.contract_infos[0].oracles.event_id,
								)
								.expect("to get an announcement");
							manager_clone
								.offer_sub_channel(
									&channel_id,
									&contract_input,
									&[vec![announcement]],
								)
								.unwrap()
						})
						.await
						.unwrap();
						dlc_message_handler.send_subchannel_message(
							pubkey,
							SubChannelMessage::Request(sub_channel_offer),
						);
					} else {
						let manager_clone = dlc_manager.clone();
						let is_contract = o == "offercontract";
						let offer = tokio::task::spawn_blocking(move || {
							if is_contract {
								DlcMessage::Offer(
									manager_clone
										.send_offer(&contract_input, pubkey)
										.expect("Error sending offer"),
								)
							} else {
								DlcMessage::OfferChannel(
									manager_clone
										.offer_channel(&contract_input, pubkey)
										.expect("Error sending offer channel"),
								)
							}
						})
						.await
						.unwrap();
						dlc_message_handler.send_message(pubkey, offer);
					}
				}
				"listoffers" => {
					for offer in dlc_manager
						.get_store()
						.get_contract_offers()
						.unwrap()
						.iter()
						.filter(|x| !x.is_offer_party)
					{
						let offer_id = hex_str(&offer.id);
						let offer_json_path = format!("{}/{}.json", offers_path, offer_id);
						if fs::metadata(&offer_json_path).is_err() {
							let offer_str = serde_json::to_string_pretty(&offer)
								.expect("Error serializing offered contract");
							fs::write(&offer_json_path, offer_str)
								.expect("Error saving offer json");
						}
						println!("Offer {:?} from {}", offer_id, offer.counter_party);
					}
				}
				a @ "acceptoffer" => {
					let contract_id = read_id_or_continue!(words, a, "contract id");

					let (_, node_id, msg) = dlc_manager
						.accept_contract_offer(&contract_id)
						.expect("Error accepting contract.");
					dlc_message_handler.send_message(node_id, DlcMessage::Accept(msg));
				}
				"listcontracts" => {
					let manager_clone = dlc_manager.clone();
					// Because the oracle client is currently blocking we need to use `spawn_blocking` here.
					tokio::task::spawn_blocking(move || {
						manager_clone.periodic_check().expect("Error doing periodic check.");
						let contracts = manager_clone
							.get_store()
							.get_contracts()
							.expect("Error retrieving contract list.");
						for contract in contracts {
							let id = hex_str(&contract.get_id());
							match contract {
								Contract::Offered(_) => {
									println!("Offered contract: {}", id);
								}
								Contract::Accepted(_) => {
									println!("Accepted contract: {}", id);
								}
								Contract::Confirmed(_) => {
									println!("Confirmed contract: {}", id);
								}
								Contract::Signed(_) => {
									println!("Signed contract: {}", id);
								}
								Contract::Closed(closed) => {
									println!("Closed contract: {}", id);
									println!(
										"Outcomes: {:?}",
										closed
											.attestations
											.iter()
											.map(|x| x.outcomes.clone())
											.collect::<Vec<_>>()
									);
									println!("PnL: {} sats", compute_pnl(&closed))
								}
								Contract::Refunded(_) => {
									println!("Refunded contract: {}", id);
								}
								_ => {
									println!("Rejected contract: {}", id);
								}
							}
						}
					})
					.await
					.expect("Error listing contract info");
				}
				"listchanneloffers" => {
					for offer in dlc_manager
						.get_store()
						.get_offered_channels()
						.unwrap()
						.iter()
						.filter(|x| !x.is_offer_party)
					{
						let channel_id = hex_str(&offer.temporary_channel_id);
						let channel_offer_json_path =
							format!("{}/{}.json", offers_path, channel_id);
						if fs::metadata(&channel_offer_json_path).is_err() {
							let offer_str = serde_json::to_string_pretty(&offer)
								.expect("Error serializing offered channel");
							fs::write(&channel_offer_json_path, offer_str)
								.expect("Error saving offer channel json");
						}
						println!("Offer channel {:?} from {}", channel_id, offer.counter_party);
					}
				}
				a @ "acceptchannel" => {
					let channel_id = read_id_or_continue!(words, a, "channel id");

					let (msg, _, _, node_id) =
						dlc_manager.accept_channel(&channel_id).expect("Error accepting channel.");
					dlc_message_handler.send_message(node_id, DlcMessage::AcceptChannel(msg));
				}
				s @ "offersettlechannel" => {
					let channel_id = read_id_or_continue!(words, s, "channel id");
					let counter_payout: u64 = match words.next().map(|w| w.parse().ok()) {
						Some(Some(p)) => p,
						_ => {
							println!("Missing or invalid counter payout parameter");
							continue;
						}
					};

					let (msg, node_id) = dlc_manager
						.settle_offer(&channel_id, counter_payout)
						.expect("Error getting settle offer message.");
					dlc_message_handler.send_message(node_id, DlcMessage::SettleOffer(msg));
				}
				l @ "acceptsettlechanneloffer" => {
					let channel_id = read_id_or_continue!(words, l, "channel id");
					let (msg, node_id) = dlc_manager
						.accept_settle_offer(&channel_id)
						.expect("Error accepting settle channel offer.");
					dlc_message_handler.send_message(node_id, DlcMessage::SettleAccept(msg));
				}
				l @ "rejectsettlechanneloffer" => {
					let channel_id = read_id_or_continue!(words, l, "channel id");
					let (msg, node_id) = dlc_manager
						.reject_settle_offer(&channel_id)
						.expect("Error rejecting settle channel offer.");
					dlc_message_handler.send_message(node_id, DlcMessage::Reject(msg));
				}
				"listsettlechanneloffers" => {
					for channel in dlc_manager
						.get_store()
						.get_signed_channels(Some(SignedChannelStateType::SettledReceived))
						.unwrap()
						.iter()
					{
						let channel_id = hex_str(&channel.channel_id);
						let own_payout = match channel.state {
							SignedChannelState::SettledReceived { own_payout, .. } => own_payout,
							_ => continue,
						};
						println!(
							"Settle offer channel {:?} from {} with own payout: {}",
							channel_id, channel.counter_party, own_payout
						);
					}
				}
				o @ "offerchannelrenew" => {
					let channel_id = read_id_or_continue!(words, o, "channel id");
					let (counter_payout, contract_path) =
						match (words.next().map(|x| x.parse()), words.next()) {
							(Some(Ok(payout)), Some(s)) => (payout, s),
							_ => continue,
						};
					let contract_input_str = fs::read_to_string(&contract_path)
						.expect("Error reading contract input file.");
					let contract_input: ContractInput = serde_json::from_str(&contract_input_str)
						.expect("Error deserializing contract input.");
					let manager_clone = dlc_manager.clone();
					let (renew_offer, node_id) = tokio::task::spawn_blocking(move || {
						manager_clone
							.renew_offer(&channel_id, counter_payout, &contract_input)
							.expect("Error sending offer")
					})
					.await
					.unwrap();
					dlc_message_handler.send_message(node_id, DlcMessage::RenewOffer(renew_offer));
				}
				"listrenewchanneloffers" => {
					for channel in dlc_manager
						.get_store()
						.get_signed_channels(Some(SignedChannelStateType::RenewOffered))
						.unwrap()
						.iter()
					{
						let channel_id = hex_str(&channel.channel_id);
						let own_payout = match channel.state {
							SignedChannelState::RenewOffered {
								counter_payout, is_offer, ..
							} => {
								if is_offer {
									continue;
								} else {
									counter_payout
								}
							}

							_ => continue,
						};
						println!(
							"Settle offer channel {:?} from {} with own payout: {}",
							channel_id, channel.counter_party, own_payout
						);
					}
				}
				l @ "acceptrenewchannel" => {
					let channel_id = read_id_or_continue!(words, l, "channel id");
					let (msg, node_id) = dlc_manager
						.accept_renew_offer(&channel_id)
						.expect("Error accepting channel.");
					dlc_message_handler.send_message(node_id, DlcMessage::RenewAccept(msg));
				}
				l @ "rejectrenewchanneloffer" => {
					let channel_id = read_id_or_continue!(words, l, "channel id");
					let (msg, node_id) = dlc_manager
						.reject_renew_offer(&channel_id)
						.expect("Error rejecting settle channel offer.");
					dlc_message_handler.send_message(node_id, DlcMessage::Reject(msg));
				}
				"listsignedchannels" => {
					for channel in dlc_manager.get_store().get_signed_channels(None).unwrap().iter()
					{
						let channel_id = hex_str(&channel.channel_id);
						println!("Signed channel {:?} with {}", channel_id, channel.counter_party);
					}
				}
				o @ "offersubchannel" => {
					let (dest_pubkey, contract_path) = match (words.next(), words.next()) {
						(Some(dest), Some(contract_path)) => {
							let dest_pk = match hex_utils::to_compressed_pubkey(dest) {
								Some(pk) => pk,
								None => {
									println!("ERROR: couldn't parse destination pubkey");
									return;
								}
							};
							(dest_pk, contract_path)
						}
						_ => {
							println!("ERROR: offersubchannel requires a destination pubkey, contract path and channel id: `offersubchannel <dest_pubkey> <contract_path> <channel_id>`");
							continue;
						}
					};
					let channel_id = read_id_or_continue!(words, o, "channel id");
					let contract_input_str = fs::read_to_string(&contract_path)
						.expect("Error reading contract input file.");
					let contract_input: ContractInput = serde_json::from_str(&contract_input_str)
						.expect("Error deserializing contract input.");
					let contract_input_clone = contract_input.clone();
					let oracle_clone = oracle.clone();
					let manager_clone = sub_channel_manager.clone();
					let sub_channel_offer = tokio::task::spawn_blocking(move || {
						let announcement = oracle_clone
							.get_announcement(
								&contract_input_clone.contract_infos[0].oracles.event_id,
							)
							.expect("to get an announcement");
						manager_clone
							.offer_sub_channel(&channel_id, &contract_input, &[vec![announcement]])
							.unwrap()
					})
					.await
					.unwrap();
					dlc_message_handler.send_subchannel_message(
						dest_pubkey,
						SubChannelMessage::Request(sub_channel_offer),
					);
				}
				"listsubchanneloffers" => {
					for c in dlc_manager.get_store().get_offered_sub_channels().unwrap().iter() {
						let channel_id = hex_str(&c.channel_id);
						println!("Sub channel offer {} with {}", channel_id, c.counter_party);
					}
				}
				a @ "acceptsubchannel" => {
					let channel_id = read_id_or_continue!(words, a, "channel id");
					let (node_id, accept_sub_channel) =
						sub_channel_manager.accept_sub_channel(&channel_id).unwrap();
					dlc_message_handler.send_subchannel_message(
						node_id,
						SubChannelMessage::Accept(accept_sub_channel),
					);
				}
				a @ "initiateforceclosesubchannel" => {
					let channel_id = read_id_or_continue!(words, a, "channel id");
					let sc = sub_channel_manager.clone();
					tokio::task::spawn_blocking(move || {
						sc.initiate_force_close_sub_channel(&channel_id).unwrap()
					})
					.await
					.unwrap();
				}
				a @ "finalizeforceclosesubchannel" => {
					let channel_id = read_id_or_continue!(words, a, "channel id");
					let sc = sub_channel_manager.clone();
					tokio::task::spawn_blocking(move || {
						sc.finalize_force_close_sub_channels(&channel_id).unwrap()
					})
					.await
					.unwrap();
				}

				_ => println!("Unknown command. See `\"help\" for available commands."),
			}
		}
		print!("> ");
		io::stdout().flush().unwrap();
	}
}

fn read_id(words: &mut SplitWhitespace, err_cmd: &str, err_arg: &str) -> Result<[u8; 32], ()> {
	match words.next() {
		None => {
			println!("ERROR: {} expects the {} as parameter.", err_cmd, err_arg);
			Err(())
		}
		Some(s) => {
			let mut res = [0u8; 32];
			match to_slice(s, &mut res) {
				Err(_) => {
					println!("ERROR: invalid {}.", err_arg);
					Err(())
				}
				Ok(_) => Ok(res),
			}
		}
	}
}

fn help() {
	println!("offercontract <pubkey@host:port> <path_to_contract_input_json>");
	println!("listoffers");
	println!("acceptoffer <contract_id>");
	println!("listcontracts");
	println!("offerchannel <pubkey@host:port> <path_to_contract_input_json>");
	println!("listchanneloffers");
	println!("acceptchannel <channel_id>");
	println!("offersettlechannel <channel_id> <counter_payout>");
	println!("listsettlechanneloffers");
	println!("acceptsettlechanneloffer <channel_id>");
	println!("rejectsettlechanneloffer <channel_id>");
	println!("offerrenewchannel <channel_id> <path_to_contract_input_json>");
	println!("listrenewchanneloffers");
	println!("acceptrenewchannel <channel_id>");
	println!("rejectrenewchannel <channel_id>");
	println!("listsignedchannels");
	println!("offersubchannel <pubkey> <path_to_contract_input_json> <ln_channel_id>");
	println!("listsubchanneloffers");
	println!("acceptsubchannel <ln_channel_id>");
	println!("initiateforceclosesubchannel <ln_channel_id>");
	println!("finalizeforceclosesubchannel <ln_channel_id>");
}

fn compute_pnl(contract: &ClosedContract) -> i64 {
	let offer = &contract.signed_contract.accepted_contract.offered_contract;
	let accepted_contract = &contract.signed_contract.accepted_contract;
	let party_params =
		if offer.is_offer_party { &offer.offer_params } else { &accepted_contract.accept_params };
	let collateral = party_params.collateral as i64;
	let cet = &contract.signed_cet;
	let v0_witness_payout_script = &party_params.payout_script_pubkey;
	let final_payout =
		cet.output
			.iter()
			.find_map(|x| {
				if &x.script_pubkey == v0_witness_payout_script {
					Some(x.value)
				} else {
					None
				}
			})
			.unwrap_or(0) as i64;
	final_payout - collateral
}

fn process_incoming_messages(
	peer_manager: &Arc<PeerManager>, dlc_manager: &Arc<DlcManager>,
	sub_channel_manager: &Arc<SubChannelManager>, dlc_message_handler: &Arc<DlcMessageHandler>,
) {
	println!("Checking for messages");
	let messages = dlc_message_handler.get_and_clear_received_messages();

	for (node_id, message) in messages {
		println!("Processing message from {}", node_id);
		let resp = dlc_manager.on_dlc_message(&message, node_id).expect("Error processing message");
		if let Some(msg) = resp {
			println!("Sending message to {}", node_id);
			dlc_message_handler.send_message(node_id, msg);
		}
	}

	let sub_channel_messages = dlc_message_handler.get_and_clear_received_sub_channel_messages();

	for (node_id, message) in sub_channel_messages {
		println!("Processing message from {}", node_id);
		let resp = sub_channel_manager
			.on_sub_channel_message(&message, &node_id)
			.expect("Error processing message");
		if let Some(msg) = resp {
			println!("Sending message to {}", node_id);
			dlc_message_handler.send_subchannel_message(node_id, msg);
		}
	}

	if dlc_message_handler.has_pending_messages() {
		peer_manager.process_events();
	}
}
