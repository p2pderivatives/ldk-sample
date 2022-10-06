use std::{convert::TryInto, net::IpAddr, str::FromStr};

use bitcoin::Network;
use lightning::ln::msgs::NetAddress;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OracleConfig {
	pub host: String,
}

#[derive(Debug)]
pub struct NetworkConfig {
	pub peer_listening_port: u16,
	pub announced_listen_addr: Option<NetAddress>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
	pub storage_dir_path: String,
	#[serde(deserialize_with = "deserialize_network_configuration")]
	pub network_configuration: NetworkConfig,
	#[serde(default)]
	pub announced_node_name: [u8; 32],
	pub network: Network,
	pub oracle_config: OracleConfig,
	pub electrs_host: String,
}

fn deserialize_network_configuration<'de, D>(deserializer: D) -> Result<NetworkConfig, D::Error>
where
	D: serde::de::Deserializer<'de>,
{
	let val = Value::deserialize(deserializer)?;

	let peer_listening_port: u16 = val["peerListeningPort"]
		.as_u64()
		.expect("Could not parse peerListeningPort")
		.try_into()
		.expect("Could not fit port in u16");

	let announced_listen_addr = if let Some(announced_listen_addr) = val.get("announcedListenAddr")
	{
		let buf = announced_listen_addr.as_str().expect("Error parsing announcedListeAddr");
		match IpAddr::from_str(buf) {
			Ok(IpAddr::V4(a)) => {
				Some(NetAddress::IPv4 { addr: a.octets(), port: peer_listening_port })
			}
			Ok(IpAddr::V6(a)) => {
				Some(NetAddress::IPv6 { addr: a.octets(), port: peer_listening_port })
			}
			Err(_) => panic!("Failed to parse announced-listen-addr into an IP address"),
		}
	} else {
		None
	};

	Ok(NetworkConfig { peer_listening_port, announced_listen_addr })
}

pub(crate) fn parse_config(config_path: &str) -> Result<Configuration, String> {
	let config_file = std::fs::read_to_string(config_path).map_err(|e| e.to_string())?;

	serde_yaml::from_str(&config_file).map_err(|e| e.to_string())
}
