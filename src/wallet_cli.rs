use std::{
	io::{BufRead, Write},
	sync::Arc,
};

use dlc_manager::Wallet;

use crate::SimpleWallet;

pub(crate) async fn poll_for_user_input(wallet: &Arc<SimpleWallet>) {
	let wallet_clone = wallet.clone();
	tokio::task::spawn_blocking(move || {
		wallet_clone.refresh().unwrap();
	})
	.await
	.unwrap();
	let stdin = std::io::stdin();
	let mut line_reader = stdin.lock().lines();
	print!("> ");
	std::io::stdout().flush().unwrap(); // Without flushing, the `>` doesn't print
	let line = match line_reader.next() {
		Some(l) => l.unwrap(),
		None => return,
	};
	let mut words = line.split_whitespace();
	if let Some(word) = words.next() {
		match word {
			"getnewaddress" => {
				let address = wallet.get_new_address().unwrap();
				println!("Receiving address: {}", address);
			}
			"getbalance" => {
				let balance = wallet.get_balance();
				println!("Balance: {}", balance);
			}
			"unreserveutxos" => {
				wallet.unreserve_all_utxos();
			}
			"emptytoaddress" => {
				let address = match words.next() {
					Some(addr_str) => addr_str.parse().unwrap(),
					None => {
						println!("Need to pass an address");
						return;
					}
				};
				let wallet_clone = wallet.clone();
				tokio::task::spawn_blocking(move || {
					wallet_clone.empty_to_address(&address).unwrap()
				})
				.await
				.unwrap();
			}
			_ => println!("Invalid command"),
		}
	}
}
