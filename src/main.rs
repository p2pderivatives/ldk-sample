pub mod bitcoind_client;
mod cli;
mod configuration;
mod convert;
mod disk;
mod dlc_cli;
mod hex_utils;
mod wallet_cli;

use crate::disk::FilesystemLogger;
use bitcoin::blockdata::constants::genesis_block;
use bitcoin::blockdata::transaction::Transaction;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::{Address, BlockHash, PackedLockTime, Script, Sequence, TxIn, TxOut, Witness};
use bitcoin_bech32::WitnessProgram;
use dlc_manager::custom_signer::{CustomKeysManager, CustomSigner};
use dlc_manager::manager::Manager;
use dlc_manager::{Blockchain, Oracle, Signer, SystemTimeProvider, Utxo, Wallet};
use dlc_messages::message_handler::MessageHandler as DlcMessageHandler;
use dlc_sled_storage_provider::SledStorageProvider;
use electrs_blockchain_provider::ElectrsBlockchainProvider;
use lightning::chain::chaininterface::{BroadcasterInterface, ConfirmationTarget, FeeEstimator};
use lightning::chain::keysinterface::{KeysInterface, KeysManager, Recipient};
use lightning::chain::{self, Listen};
use lightning::chain::{chainmonitor, ChannelMonitorUpdateStatus};
use lightning::chain::{BestBlock, Filter, Watch};
use lightning::ln::channelmanager;
use lightning::ln::channelmanager::{ChainParameters, ChannelManagerReadArgs};
use lightning::ln::peer_handler::{IgnoringMessageHandler, MessageHandler};
use lightning::ln::{PaymentHash, PaymentPreimage, PaymentSecret};
use lightning::routing::gossip;
use lightning::routing::gossip::{NodeId, P2PGossipSync};
use lightning::routing::scoring::ProbabilisticScorer;
use lightning::util::config::UserConfig;
use lightning::util::events::{Event, EventsProvider, PaymentPurpose};
use lightning::util::ser::ReadableArgs;
use lightning_background_processor::BackgroundProcessor;
use lightning_block_sync::poll;
use lightning_block_sync::UnboundedCache;
use lightning_block_sync::{init, BlockSource};
use lightning_invoice::payment;
use lightning_invoice::utils::DefaultRouter;
use lightning_net_tokio::SocketDescriptor;
use lightning_persister::FilesystemPersister;
use lightning_rapid_gossip_sync::RapidGossipSync;
use p2pd_oracle_client::P2PDOracleClient;
use rand::{thread_rng, Rng};
use simple_wallet::WalletStorage;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::convert::TryInto;
use std::fmt;
use std::fs;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub(crate) enum HTLCStatus {
	Pending,
	Succeeded,
	Failed,
}

pub(crate) struct MillisatAmount(Option<u64>);

impl fmt::Display for MillisatAmount {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match self.0 {
			Some(amt) => write!(f, "{}", amt),
			None => write!(f, "unknown"),
		}
	}
}

pub(crate) struct PaymentInfo {
	preimage: Option<PaymentPreimage>,
	secret: Option<PaymentSecret>,
	status: HTLCStatus,
	amt_msat: MillisatAmount,
}

pub(crate) type PaymentInfoStorage = Arc<Mutex<HashMap<PaymentHash, PaymentInfo>>>;

type ChainMonitor = chainmonitor::ChainMonitor<
	CustomSigner,
	Arc<dyn Filter + Send + Sync>,
	Arc<ElectrsBlockchainProvider>,
	Arc<ElectrsBlockchainProvider>,
	Arc<FilesystemLogger>,
	Arc<FilesystemPersister>,
>;

pub(crate) type PeerManager = lightning::ln::peer_handler::PeerManager<
	SocketDescriptor,
	Arc<ChannelManager>,
	Arc<
		P2PGossipSync<
			Arc<lightning::routing::gossip::NetworkGraph<Arc<FilesystemLogger>>>,
			Arc<dyn chain::Access + Send + Sync>,
			Arc<FilesystemLogger>,
		>,
	>,
	Arc<IgnoringMessageHandler>,
	Arc<FilesystemLogger>,
	Arc<DlcMessageHandler>,
>;

pub(crate) type ChannelManager = lightning::ln::channelmanager::ChannelManager<
	Arc<ChainMonitor>,
	Arc<ElectrsBlockchainProvider>,
	Arc<CustomKeysManager>,
	Arc<ElectrsBlockchainProvider>,
	Arc<FilesystemLogger>,
>;

pub(crate) type InvoicePayer<E> =
	payment::InvoicePayer<Arc<ChannelManager>, Router, Arc<FilesystemLogger>, E>;

type Router = DefaultRouter<
	Arc<NetworkGraph>,
	Arc<FilesystemLogger>,
	Arc<Mutex<ProbabilisticScorer<Arc<NetworkGraph>, Arc<FilesystemLogger>>>>,
>;

pub(crate) type NetworkGraph = gossip::NetworkGraph<Arc<FilesystemLogger>>;

type GossipSync<P, G, A, L> =
	lightning_background_processor::GossipSync<P, Arc<RapidGossipSync<G, L>>, G, A, L>;

type SimpleWallet =
	simple_wallet::SimpleWallet<Arc<ElectrsBlockchainProvider>, Arc<SledStorageProvider>>;

type DlcManager = Manager<
	Arc<SimpleWallet>,
	Arc<ElectrsBlockchainProvider>,
	Arc<SledStorageProvider>,
	Arc<P2PDOracleClient>,
	Arc<SystemTimeProvider>,
	Arc<ElectrsBlockchainProvider>,
>;

type SubChannelManager = dlc_manager::sub_channel_manager::SubChannelManager<
	Arc<SimpleWallet>,
	Arc<ChannelManager>,
	Arc<SledStorageProvider>,
	Arc<ElectrsBlockchainProvider>,
	Arc<P2PDOracleClient>,
	Arc<SystemTimeProvider>,
	Arc<ElectrsBlockchainProvider>,
	Arc<
		Manager<
			Arc<SimpleWallet>,
			Arc<ElectrsBlockchainProvider>,
			Arc<SledStorageProvider>,
			Arc<P2PDOracleClient>,
			Arc<SystemTimeProvider>,
			Arc<ElectrsBlockchainProvider>,
		>,
	>,
>;

struct NodeAlias<'a>(&'a [u8; 32]);

impl fmt::Display for NodeAlias<'_> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		let alias = self
			.0
			.iter()
			.map(|b| *b as char)
			.take_while(|c| *c != '\0')
			.filter(|c| c.is_ascii_graphic() || *c == ' ')
			.collect::<String>();
		write!(f, "{}", alias)
	}
}

async fn handle_ldk_events(
	channel_manager: &Arc<ChannelManager>, electrs: &Arc<ElectrsBlockchainProvider>,
	network_graph: &NetworkGraph, keys_manager: Arc<CustomKeysManager>,
	inbound_payments: &PaymentInfoStorage, outbound_payments: &PaymentInfoStorage, event: &Event,
	wallet: &Arc<SimpleWallet>,
) {
	match event {
		Event::FundingGenerationReady {
			temporary_channel_id,
			counterparty_node_id,
			channel_value_satoshis,
			output_script,
			..
		} => {
			// Construct the raw transaction with one output, that is paid the amount of the
			// channel.
			let addr = WitnessProgram::from_scriptpubkey(
				&output_script[..],
				bitcoin_bech32::constants::Network::Regtest,
			)
			.expect("Lightning funding tx should always be to a SegWit output")
			.to_address();
			let address: Address = addr.parse().unwrap();
			let mut tx = Transaction {
				version: 2,
				lock_time: PackedLockTime::ZERO,
				input: vec![TxIn::default()],
				output: vec![TxOut {
					value: *channel_value_satoshis,
					script_pubkey: address.script_pubkey(),
				}],
			};

			let fee_rate = electrs.get_est_sat_per_1000_weight(ConfirmationTarget::Normal) as u64;
			let fee = u64::max(((tx.weight() + tx.input.len() * 74) as u64) * fee_rate / 1000, 153);

			let required_amount = *channel_value_satoshis + fee;

			let utxos: Vec<Utxo> =
				wallet.get_utxos_for_amount(required_amount, None, false).unwrap();

			tx.input = Vec::new();

			let change_address = wallet.get_new_address().unwrap();

			tx.output.push(TxOut {
				value: utxos.iter().map(|x| x.tx_out.value).sum::<u64>() - required_amount,
				script_pubkey: change_address.script_pubkey(),
			});

			for (i, utxo) in utxos.iter().enumerate() {
				tx.input.push(TxIn {
					previous_output: utxo.outpoint.clone(),
					script_sig: Script::default(),
					sequence: Sequence::MAX,
					witness: Witness::default(),
				});
				wallet.sign_tx_input(&mut tx, i, &utxo.tx_out, None).unwrap();
			}

			// Give the funding transaction back to LDK for opening the channel.
			channel_manager
				.funding_transaction_generated(&temporary_channel_id, counterparty_node_id, tx)
				.unwrap();
		}
		Event::PaymentReceived { payment_hash, purpose, amount_msat } => {
			println!(
				"\nEVENT: received payment from payment hash {} of {} millisatoshis",
				hex_utils::hex_str(&payment_hash.0),
				amount_msat,
			);
			print!("> ");
			io::stdout().flush().unwrap();
			let payment_preimage = match purpose {
				PaymentPurpose::InvoicePayment { payment_preimage, .. } => *payment_preimage,
				PaymentPurpose::SpontaneousPayment(preimage) => Some(*preimage),
			};
			channel_manager.claim_funds(payment_preimage.unwrap());
		}
		Event::PaymentClaimed { payment_hash, purpose, amount_msat } => {
			println!(
				"\nEVENT: claimed payment from payment hash {} of {} millisatoshis",
				hex_utils::hex_str(&payment_hash.0),
				amount_msat,
			);
			print!("> ");
			io::stdout().flush().unwrap();
			let (payment_preimage, payment_secret) = match purpose {
				PaymentPurpose::InvoicePayment { payment_preimage, payment_secret, .. } => {
					(*payment_preimage, Some(*payment_secret))
				}
				PaymentPurpose::SpontaneousPayment(preimage) => (Some(*preimage), None),
			};
			let mut payments = inbound_payments.lock().unwrap();
			match payments.entry(*payment_hash) {
				Entry::Occupied(mut e) => {
					let payment = e.get_mut();
					payment.status = HTLCStatus::Succeeded;
					payment.preimage = payment_preimage;
					payment.secret = payment_secret;
				}
				Entry::Vacant(e) => {
					e.insert(PaymentInfo {
						preimage: payment_preimage,
						secret: payment_secret,
						status: HTLCStatus::Succeeded,
						amt_msat: MillisatAmount(Some(*amount_msat)),
					});
				}
			}
		}
		Event::PaymentSent { payment_preimage, payment_hash, fee_paid_msat, .. } => {
			let mut payments = outbound_payments.lock().unwrap();
			for (hash, payment) in payments.iter_mut() {
				if *hash == *payment_hash {
					payment.preimage = Some(*payment_preimage);
					payment.status = HTLCStatus::Succeeded;
					println!(
						"\nEVENT: successfully sent payment of {} millisatoshis{} from \
								 payment hash {:?} with preimage {:?}",
						payment.amt_msat,
						if let Some(fee) = fee_paid_msat {
							format!(" (fee {} msat)", fee)
						} else {
							"".to_string()
						},
						hex_utils::hex_str(&payment_hash.0),
						hex_utils::hex_str(&payment_preimage.0)
					);
					print!("> ");
					io::stdout().flush().unwrap();
				}
			}
		}
		Event::OpenChannelRequest { .. } => {
			// Unreachable, we don't set manually_accept_inbound_channels
		}
		Event::PaymentPathSuccessful { .. } => {}
		Event::PaymentPathFailed { .. } => {}
		Event::PaymentFailed { payment_hash, .. } => {
			print!(
				"\nEVENT: Failed to send payment to payment hash {:?}: exhausted payment retry attempts",
				hex_utils::hex_str(&payment_hash.0)
			);
			print!("> ");
			io::stdout().flush().unwrap();

			let mut payments = outbound_payments.lock().unwrap();
			if payments.contains_key(&payment_hash) {
				let payment = payments.get_mut(&payment_hash).unwrap();
				payment.status = HTLCStatus::Failed;
			}
		}
		Event::PaymentForwarded {
			prev_channel_id,
			next_channel_id,
			fee_earned_msat,
			claim_from_onchain_tx,
		} => {
			let read_only_network_graph = network_graph.read_only();
			let nodes = read_only_network_graph.nodes();
			let channels = channel_manager.list_channels();

			let node_str = |channel_id: &Option<[u8; 32]>| match channel_id {
				None => String::new(),
				Some(channel_id) => match channels.iter().find(|c| c.channel_id == *channel_id) {
					None => String::new(),
					Some(channel) => {
						match nodes.get(&NodeId::from_pubkey(&channel.counterparty.node_id)) {
							None => " from private node".to_string(),
							Some(node) => match &node.announcement_info {
								None => " from unnamed node".to_string(),
								Some(announcement) => {
									format!("node {}", announcement.alias)
								}
							},
						}
					}
				},
			};
			let channel_str = |channel_id: &Option<[u8; 32]>| {
				channel_id
					.map(|channel_id| format!(" with channel {}", hex_utils::hex_str(&channel_id)))
					.unwrap_or_default()
			};
			let from_prev_str =
				format!("{}{}", node_str(prev_channel_id), channel_str(prev_channel_id));
			let to_next_str =
				format!("{}{}", node_str(next_channel_id), channel_str(next_channel_id));

			let from_onchain_str = if *claim_from_onchain_tx {
				"from onchain downstream claim"
			} else {
				"from HTLC fulfill message"
			};
			if let Some(fee_earned) = fee_earned_msat {
				println!(
					"\nEVENT: Forwarded payment{}{}, earning {} msat {}",
					from_prev_str, to_next_str, fee_earned, from_onchain_str
				);
			} else {
				println!(
					"\nEVENT: Forwarded payment{}{}, claiming onchain {}",
					from_prev_str, to_next_str, from_onchain_str
				);
			}
			print!("> ");
			io::stdout().flush().unwrap();
		}
		Event::PendingHTLCsForwardable { time_forwardable } => {
			let forwarding_channel_manager = channel_manager.clone();
			let min = time_forwardable.as_millis() as u64;
			tokio::spawn(async move {
				let millis_to_sleep = thread_rng().gen_range(min, min * 5) as u64;
				tokio::time::sleep(Duration::from_millis(millis_to_sleep)).await;
				forwarding_channel_manager.process_pending_htlc_forwards();
			});
		}
		Event::SpendableOutputs { outputs } => {
			let destination_address = wallet.get_new_address().unwrap();
			let output_descriptors = &outputs.iter().map(|a| a).collect::<Vec<_>>();
			let tx_feerate = electrs.get_est_sat_per_1000_weight(ConfirmationTarget::Normal);
			let spending_tx = keys_manager
				.spend_spendable_outputs(
					output_descriptors,
					Vec::new(),
					destination_address.script_pubkey(),
					tx_feerate,
					&Secp256k1::new(),
				)
				.unwrap();
			electrs.broadcast_transaction(&spending_tx);
		}
		Event::ChannelClosed { channel_id, reason, user_channel_id: _ } => {
			println!(
				"\nEVENT: Channel {} closed due to: {:?}",
				hex_utils::hex_str(channel_id),
				reason
			);
			print!("> ");
			io::stdout().flush().unwrap();
		}
		Event::DiscardFunding { .. } => {
			// A "real" node should probably "lock" the UTXOs spent in funding transactions until
			// the funding transaction either confirms, or this event is generated.
		}
		Event::ProbeSuccessful { .. } => {}
		Event::ProbeFailed { .. } => {}
		Event::ChannelReady { .. } => {}
		Event::HTLCHandlingFailed { .. } => {}
	}
}

async fn start_ldk() {
	let mut args = std::env::args();
	if args.len() != 2 {
		println!("This application requires a single argument corresponding to the path to a configuration file.");
		return;
	}

	// Parse application configuration
	let config =
		configuration::parse_config(&args.nth(1).unwrap()).expect("Error parsing arguments");
	fs::create_dir_all(&config.storage_dir_path).expect("Error creating storage directory.");
	let offers_path = format!("{}/{}", config.storage_dir_path, "offers");
	fs::create_dir_all(&offers_path).expect("Error creating offered contract directory");

	// Initialize the LDK data directory if necessary.
	let ldk_data_dir = format!("{}/.ldk", config.storage_dir_path);
	fs::create_dir_all(ldk_data_dir.clone()).unwrap();

	// ## Setup
	let logger = Arc::new(FilesystemLogger::new(ldk_data_dir.clone()));
	let host = config.electrs_host.clone();
	let network = config.network;
	let electrs = tokio::task::spawn_blocking(move || {
		Arc::new(ElectrsBlockchainProvider::new(host, network))
	})
	.await
	.unwrap();

	let persister = Arc::new(FilesystemPersister::new(ldk_data_dir.clone()));

	let chain_monitor: Arc<ChainMonitor> = Arc::new(chainmonitor::ChainMonitor::new(
		None,
		electrs.clone(),
		logger.clone(),
		electrs.clone(),
		persister.clone(),
	));

	// Step 6: Initialize the KeysManager

	// The key seed that we use to derive the node privkey (that corresponds to the node pubkey) and
	// other secret key material.
	let keys_seed_path = format!("{}/keys_seed", ldk_data_dir.clone());
	let keys_seed = if let Ok(seed) = fs::read(keys_seed_path.clone()) {
		assert_eq!(seed.len(), 32);
		let mut key = [0; 32];
		key.copy_from_slice(&seed);
		key
	} else {
		let mut key = [0; 32];
		thread_rng().fill_bytes(&mut key);
		match File::create(keys_seed_path.clone()) {
			Ok(mut f) => {
				f.write_all(&key).expect("Failed to write node keys seed to disk");
				f.sync_all().expect("Failed to sync node keys seed to disk");
			}
			Err(e) => {
				println!("ERROR: Unable to create keys seed file {}: {}", keys_seed_path, e);
				return;
			}
		}
		key
	};
	let cur = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
	let keys_manager = KeysManager::new(&keys_seed, cur.as_secs(), cur.subsec_nanos());
	let keys_manager = Arc::new(CustomKeysManager::new(keys_manager));

	let mut channelmonitors = persister.read_channelmonitors(keys_manager.clone()).unwrap();

	let mut user_config = UserConfig {
		channel_handshake_config: lightning::util::config::ChannelHandshakeConfig {
			max_inbound_htlc_value_in_flight_percent_of_channel: 50,
			..Default::default()
		},
		..Default::default()
	};
	user_config.channel_handshake_limits.force_announced_channel_preference = false;
	let mut restarting_node = true;
	let (channel_manager_blockhash, channel_manager) = {
		if let Ok(mut f) = fs::File::open(format!("{}/manager", ldk_data_dir.clone())) {
			let mut channel_monitor_mut_references = Vec::new();
			for (_, channel_monitor) in channelmonitors.iter_mut() {
				channel_monitor_mut_references.push(channel_monitor);
			}
			let read_args = ChannelManagerReadArgs::new(
				keys_manager.clone(),
				electrs.clone(),
				chain_monitor.clone(),
				electrs.clone(),
				logger.clone(),
				user_config,
				channel_monitor_mut_references,
			);
			<(BlockHash, ChannelManager)>::read(&mut f, read_args).unwrap()
		} else {
			// We're starting a fresh node.
			restarting_node = false;
			let (block_hash, block_height) = electrs.get_best_block().await.unwrap();

			let chain_params = ChainParameters {
				network: config.network,
				best_block: BestBlock::new(block_hash, block_height.unwrap()),
			};
			let fresh_channel_manager = channelmanager::ChannelManager::new(
				electrs.clone(),
				chain_monitor.clone(),
				electrs.clone(),
				logger.clone(),
				keys_manager.clone(),
				user_config,
				chain_params,
			);
			(block_hash, fresh_channel_manager)
		}
	};

	println!("Our Node ID: {}", channel_manager.get_our_node_id());

	// Step 9: Sync ChannelMonitors and ChannelManager to chain tip
	let mut chain_listener_channel_monitors = Vec::new();
	let mut cache = UnboundedCache::new();
	let mut chain_tip: Option<poll::ValidatedBlockHeader> = None;
	if restarting_node {
		let mut chain_listeners =
			vec![(channel_manager_blockhash, &channel_manager as &dyn chain::Listen)];

		for (blockhash, channel_monitor) in channelmonitors.drain(..) {
			let outpoint = channel_monitor.get_original_funding_txo().0;
			chain_listener_channel_monitors.push((
				blockhash,
				(channel_monitor, electrs.clone(), electrs.clone(), logger.clone()),
				outpoint,
			));
		}

		for monitor_listener_info in chain_listener_channel_monitors.iter_mut() {
			chain_listeners
				.push((monitor_listener_info.0, &monitor_listener_info.1 as &dyn chain::Listen));
		}

		chain_tip = Some(
			init::synchronize_listeners(
				electrs.clone(),
				config.network,
				&mut cache,
				chain_listeners,
			)
			.await
			.unwrap(),
		);
	}

	// Step 10: Give ChannelMonitors to ChainMonitor
	for item in chain_listener_channel_monitors.drain(..) {
		let channel_monitor = item.1 .0;
		let funding_outpoint = item.2;
		assert_eq!(
			chain_monitor.watch_channel(funding_outpoint, channel_monitor),
			ChannelMonitorUpdateStatus::Completed
		);
	}

	// Step 11: Optional: Initialize the P2PGossipSync
	let genesis = genesis_block(config.network).header.block_hash();
	let network_graph_path = format!("{}/network_graph", ldk_data_dir.clone());
	let network_graph =
		Arc::new(disk::read_network(Path::new(&network_graph_path), genesis, logger.clone()));
	let gossip_sync = Arc::new(P2PGossipSync::new(
		Arc::clone(&network_graph),
		None::<Arc<dyn chain::Access + Send + Sync>>,
		logger.clone(),
	));

	// Step 12: Initialize the PeerManager
	let channel_manager: Arc<ChannelManager> = Arc::new(channel_manager);
	let mut ephemeral_bytes = [0; 32];
	rand::thread_rng().fill_bytes(&mut ephemeral_bytes);
	let lightning_msg_handler = MessageHandler {
		chan_handler: channel_manager.clone(),
		route_handler: gossip_sync.clone(),
		onion_message_handler: Arc::new(IgnoringMessageHandler {}),
	};
	let dlc_message_handler = Arc::new(DlcMessageHandler::new());
	let current_time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();

	let peer_manager: Arc<PeerManager> = Arc::new(PeerManager::new(
		lightning_msg_handler,
		keys_manager.get_node_secret(Recipient::Node).unwrap(),
		current_time.try_into().unwrap(),
		&ephemeral_bytes,
		logger.clone(),
		dlc_message_handler.clone(),
	));

	// ## Running LDK
	// Step 13: Initialize networking

	let peer_manager_connection_handler = peer_manager.clone();
	let listening_port = config.network_configuration.peer_listening_port;
	let stop_listen_connect = Arc::new(AtomicBool::new(false));
	let stop_listen = Arc::clone(&stop_listen_connect);
	tokio::spawn(async move {
		let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", listening_port))
			.await
			.expect("Failed to bind to listen port - is something else already listening on it?");
		loop {
			let peer_mgr = peer_manager_connection_handler.clone();
			let tcp_stream = listener.accept().await.unwrap().0;
			if stop_listen.load(Ordering::Acquire) {
				return;
			}
			tokio::spawn(async move {
				lightning_net_tokio::setup_inbound(
					peer_mgr.clone(),
					tcp_stream.into_std().unwrap(),
				)
				.await;
			});
		}
	});

	// Step 15: Handle LDK Events
	let channel_manager_event_listener = channel_manager.clone();
	let keys_manager_listener = keys_manager.clone();
	// TODO: persist payment info to disk
	let inbound_payments: PaymentInfoStorage = Arc::new(Mutex::new(HashMap::new()));
	let outbound_payments: PaymentInfoStorage = Arc::new(Mutex::new(HashMap::new()));
	let inbound_pmts_for_events = inbound_payments.clone();
	let outbound_pmts_for_events = outbound_payments.clone();
	let network_graph_events = network_graph.clone();
	let handle = tokio::runtime::Handle::current();
	let electrs_clone = electrs.clone();
	let storage = Arc::new(
		SledStorageProvider::new(&ldk_data_dir).expect("to be able to create sled storage"),
	);

	let wallet = Arc::new(SimpleWallet::new(electrs.clone(), storage.clone(), config.network));
	let wallet_clone = wallet.clone();

	let event_handler = move |event: Event| {
		handle.block_on(handle_ldk_events(
			&channel_manager_event_listener,
			&electrs_clone,
			&network_graph_events,
			keys_manager_listener.clone(),
			&inbound_pmts_for_events,
			&outbound_pmts_for_events,
			&event,
			&wallet_clone,
		));
	};

	// Step 14: Connect and Disconnect Blocks
	if chain_tip.is_none() {
		chain_tip = Some(init::validate_best_block_header(electrs.clone()).await.unwrap());
	}
	let chain_tip = chain_tip.unwrap();
	let channel_manager_listener = channel_manager.clone();
	let chain_monitor_listener = chain_monitor.clone();
	let electrs_clone = electrs.clone();
	let peer_manager_clone = peer_manager.clone();
	let event_handler_clone = event_handler.clone();
	let logger_clone = logger.clone();
	std::thread::spawn(move || {
		let mut last_height = chain_tip.height as u64;
		loop {
			let chain_tip_height = electrs_clone.get_blockchain_height().unwrap();
			for i in last_height + 1..=chain_tip_height {
				let block = electrs_clone.get_block_at_height(i).unwrap();
				channel_manager_listener.block_connected(&block, i as u32);
				for ftxo in chain_monitor_listener.list_monitors() {
					chain_monitor_listener.get_monitor(ftxo).unwrap().block_connected(
						&block.header,
						&block.txdata.iter().enumerate().collect::<Vec<_>>(),
						i as u32,
						electrs_clone.clone(),
						electrs_clone.clone(),
						logger_clone.clone(),
					);
				}
			}
			last_height = chain_tip_height;
			channel_manager_listener.process_pending_events(&event_handler_clone);
			chain_monitor_listener.process_pending_events(&event_handler_clone);
			peer_manager_clone.process_events();
			std::thread::sleep(Duration::from_secs(1));
		}
	});

	// Step 16: Initialize routing ProbabilisticScorer
	let scorer_path = format!("{}/scorer", ldk_data_dir.clone());
	let scorer = Arc::new(Mutex::new(disk::read_scorer(
		Path::new(&scorer_path),
		Arc::clone(&network_graph),
		Arc::clone(&logger),
	)));

	// Step 17: Create InvoicePayer
	let router = DefaultRouter::new(
		network_graph.clone(),
		logger.clone(),
		keys_manager.get_secure_random_bytes(),
		scorer.clone(),
	);
	let invoice_payer = Arc::new(InvoicePayer::new(
		channel_manager.clone(),
		router,
		logger.clone(),
		event_handler.clone(),
		payment::Retry::Timeout(Duration::from_secs(10)),
	));

	// Step 18: Persist ChannelManager and NetworkGraph
	let persister = Arc::new(FilesystemPersister::new(ldk_data_dir.clone()));

	// Step 19: Background Processing
	let background_processor = BackgroundProcessor::start(
		persister,
		invoice_payer.clone(),
		chain_monitor.clone(),
		channel_manager.clone(),
		GossipSync::P2P(gossip_sync.clone()),
		peer_manager.clone(),
		logger.clone(),
		Some(scorer.clone()),
	);

	// Regularly reconnect to channel peers.
	let connect_cm = Arc::clone(&channel_manager);
	let connect_pm = Arc::clone(&peer_manager);
	let peer_data_path = format!("{}/channel_peer_data", ldk_data_dir.clone());
	let stop_connect = Arc::clone(&stop_listen_connect);
	tokio::spawn(async move {
		let mut interval = tokio::time::interval(Duration::from_secs(1));
		loop {
			interval.tick().await;
			match disk::read_channel_peer_data(Path::new(&peer_data_path)) {
				Ok(info) => {
					let peers = connect_pm.get_peer_node_ids();
					for node_id in connect_cm
						.list_channels()
						.iter()
						.map(|chan| chan.counterparty.node_id)
						.filter(|id| !peers.contains(id))
					{
						if stop_connect.load(Ordering::Acquire) {
							return;
						}
						for (pubkey, peer_addr) in info.iter() {
							if *pubkey == node_id {
								let _ = cli::do_connect_peer(
									*pubkey,
									peer_addr.clone(),
									Arc::clone(&connect_pm),
								)
								.await;
							}
						}
					}
				}
				Err(e) => println!("ERROR: errored reading channel peer info from disk: {:?}", e),
			}
		}
	});

	// Regularly broadcast our node_announcement. This is only required (or possible) if we have
	// some public channels, and is only useful if we have public listen address(es) to announce.
	// In a production environment, this should occur only after the announcement of new channels
	// to avoid churn in the global network graph.
	let network = config.network;
	// if !args.ldk_announced_listen_addr.is_empty() {
	// 	tokio::spawn(async move {
	// 		let mut interval = tokio::time::interval(Duration::from_secs(60));
	// 		loop {
	// 			interval.tick().await;
	// 			chan_manager.broadcast_node_announcement(
	// 				[0; 3],
	// 				args.ldk_announced_node_name,
	// 				args.ldk_announced_listen_addr.clone(),
	// 			);
	// 		}
	// 	});
	// }

	let p2pdoracle = tokio::task::spawn_blocking(move || {
		Arc::new(
			P2PDOracleClient::new("https://oracle.p2pderivatives.io/")
				.expect("to be able to create the p2pd oracle"),
		)
	})
	.await
	.unwrap();

	let oracle_pubkey = p2pdoracle.get_public_key();

	let oracles = HashMap::from([(oracle_pubkey, p2pdoracle.clone())]);

	let wallet_clone = wallet.clone();
	let electrs_clone = electrs.clone();

	let addresses = storage.get_addresses().unwrap();
	for address in addresses {
		println!("{}", address);
	}

	let store_clone = storage.clone();

	let dlc_manager = tokio::task::spawn_blocking(move || {
		Arc::new(
			Manager::new(
				wallet_clone,
				electrs_clone.clone(),
				store_clone,
				oracles,
				Arc::new(SystemTimeProvider {}),
				electrs_clone,
			)
			.unwrap(),
		)
	})
	.await
	.unwrap();

	let sub_channel_manager = Arc::new(SubChannelManager::new(
		Secp256k1::new(),
		wallet.clone(),
		channel_manager.clone(),
		storage,
		electrs.clone(),
		dlc_manager.clone(),
	));

	loop {
		println!("Enter 1 for LN functions, 2 for DLC, 3 for wallet");
		print!(">");
		io::stdout().flush().unwrap(); // Without flushing, the `>` doesn't print

		let mut input_line = String::new();
		io::stdin().read_line(&mut input_line).expect("Failed to read line");
		if input_line.is_empty() {
			break;
		}

		let choice = input_line.trim().parse();

		match choice {
			Err(_) => println!("Invalid input"),
			Ok(c) => match c {
				0 => break,
				1 => {
					cli::poll_for_user_input(
						Arc::clone(&invoice_payer),
						Arc::clone(&peer_manager),
						Arc::clone(&channel_manager),
						Arc::clone(&keys_manager),
						Arc::clone(&network_graph),
						inbound_payments.clone(),
						outbound_payments.clone(),
						ldk_data_dir.clone(),
						network,
						logger.clone(),
					)
					.await
				}
				2 => {
					dlc_cli::poll_for_user_input(
						peer_manager.clone(),
						dlc_message_handler.clone(),
						dlc_manager.clone(),
						sub_channel_manager.clone(),
						p2pdoracle.clone(),
						&offers_path,
					)
					.await
				}
				3 => {
					wallet_cli::poll_for_user_input(&wallet).await;
				}
				_ => {
					println!("Invalid choice");
				}
			},
		}
	}

	// Disconnect our peers and stop accepting new connections. This ensures we don't continue
	// updating our channel data after we've stopped the background processor.
	stop_listen_connect.store(true, Ordering::Release);
	peer_manager.disconnect_all_peers();

	// Stop the background processor.
	background_processor.stop().unwrap();
}

#[tokio::main]
pub async fn main() {
	start_ldk().await;
}
