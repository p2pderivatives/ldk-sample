# ldk-sample
Sample node implementation using LDK and rust-dlc, supporting DLC within Lightning channels.

## Quick run on regtest

Requires docker and docker-compose.

```bash
docker-compose up -d

bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest generatetoaddress 200 $(bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest getnewaddress)

cargo run ./examples/configurations/alice.yml
```

In a different terminal:

```bash
cargo run ./examples/configurations/bob.yml
```

Back to the first terminal where the CLI is running:

```
Enter 1 for LN functions, 2 for DLC, 3 for wallet
>3
> getnewaddress
Receiving address: xxxxxxxx
Enter 1 for LN functions, 2 for DLC, 3 for wallet
>
```

Yet in another terminal:
```
bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest sendtoaddress xxxxxxxx 0.002
```
(using the address you generated previously)

Back to the first terminal:
```
Enter 1 for LN functions, 2 for DLC, 3 for wallet
>3
> getbalance
Balance: 200000
Enter 1 for LN functions, 2 for DLC, 3 for wallet
>1
LDK startup successful. To view available commands: "help".
LDK logs are available at <your-supplied-ldk-data-dir-path>/.ldk/logs
Local Node ID is 025ba196b3257938b02032f806d1ef6edd8403b5a3a5df8b5549429090e615a9ac.
> openchannel 02f7486899f42946a19d5f5bbdfb537d91e200dfda85408b014600a18255e53014@127.0.0.1:9001 180000
EVENT: initiated channel with peer 02f7486899f42946a19d5f5bbdfb537d91e200dfda85408b014600a18255e53014.
Enter 1 for LN functions, 2 for DLC, 3 for wallet
>
```
(replace the node id with that of the second instance)

Generate some blocks in another terminal:
```
bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest generatetoaddress 20 $(bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest getnewaddress)
```

In the first terminal check that the channel is confirmed and do a keysend to the second node:
```
Enter 1 for LN functions, 2 for DLC, 3 for wallet
> 1
LDK startup successful. To view available commands: "help".
LDK logs are available at <your-supplied-ldk-data-dir-path>/.ldk/logs
Local Node ID is 025ba196b3257938b02032f806d1ef6edd8403b5a3a5df8b5549429090e615a9ac.
> listchannels
[
        {
                channel_id: bc7121c4d2c809777930be4d82a944139fc815d010184ef20e4bfe72c20a17ca,
                funding_txid: ca170ac272fe4b0ef24e1810d015c89f1344a9824dbe30797709c8d2c42171bc,
                peer_pubkey: 02f7486899f42946a19d5f5bbdfb537d91e200dfda85408b014600a18255e53014,
                short_channel_id: 231996953591808,
                is_channel_ready: true,
                channel_value_satoshis: 180000,
                local_balance_msat: 180000000,
                available_balance_for_send_msat: 178200000,
                available_balance_for_recv_msat: 0,
                channel_can_send_payments: true,
                public: false,
Outbound capacity msat 178200000
        },
]
Enter 1 for LN functions, 2 for DLC, 3 for wallet
> 1
> keysend 02f7486899f42946a19d5f5bbdfb537d91e200dfda85408b014600a18255e53014 90000000
EVENT: initiated sending 90000000 msats to 02f7486899f42946a19d5f5bbdfb537d91e200dfda85408b014600a18255e53014
> Enter 1 for LN functions, 2 for DLC, 3 for wallet
>
EVENT: successfully sent payment of 90000000 millisatoshis (fee 0 msat) from payment hash "a5fd1b7544dc5794cb10869e5a7f676c68d37b472d27bfe028986275b05e67c5" with preimage "f9a7957e4946d082f5822586129d221691b34be5e2091d0e4e8c0993180621b9"
```

In the same terminal we can now offer to open a "subchannel" with a DLC in it:
```
Enter 1 for LN functions, 2 for DLC, 3 for wallet
>2
To view available commands: "help".
> offersubchannel 02f7486899f42946a19d5f5bbdfb537d91e200dfda85408b014600a18255e53014 ./examples/contracts/numerical_contract_input.json bc7121c4d2c809777930be4d82a944139fc815d010184ef20e4bfe72c20a17ca
Checking for messages
```

Remember to use the proper node id and channel id.

In the second instance we accept the offer:
```
Enter 1 for LN functions, 2 for DLC, 3 for wallet
>2
To view available commands: "help".
> listsubchanneloffers
Checking for messages
Sub channel offer bc7121c4d2c809777930be4d82a944139fc815d010184ef20e4bfe72c20a17ca with 025ba196b3257938b02032f806d1ef6edd8403b5a3a5df8b5549429090e615a9ac
> acceptsubchannel bc7121c4d2c809777930be4d82a944139fc815d010184ef20e4bfe72c20a17ca
Checking for messages
>
```

To process messages to be exchanged, just enter anything (like "2") while under the DLC sub-menu on both sides.
Once the sub-channel is set up, we can force close it as follows:

```
initiateforceclosesubchannel bc7121c4d2c809777930be4d82a944139fc815d010184ef20e4bfe72c20a17ca
```

Then generate at least 288 blocks in another terminal:
```
$bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest generatetoaddress 289 $(bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest getnewaddress)
```

Finalize the force closing:
```
finalizeforceclosesubchannel bc7121c4d2c809777930be4d82a944139fc815d010184ef20e4bfe72c20a17ca
```

Finalize the closing by generating more blocks:
```
$bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest generatetoaddress 289 $(bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest getnewaddress)
```

Then navigating to the DLC sub-menu should trigger the sending of the CET to close the DLC channel.

## License

Licensed under either:

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT License ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
