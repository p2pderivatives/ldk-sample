# ldk-sample
Sample node implementation using LDK and rust-dlc, enabling "stabilizing" part of the fund of a lightning channel, pegging its value to USD.

## Functionalities

Two new commands are available under the LN sub-menu:
* stabilizechannel that takes a channel ID and the collateral to be stabilized for each party
* unstabilizechannel that will return the channel to a "regular" LN one.

Once a channel is stabilized, the underlying CFD contract will be automatically renewed 2 minutes before the contract expiry.
Contract maturity is set to 10 minutes after contract creation (in practice this could/should be days or weeks).

The fund that are not "stabilized" can still be used for Lightning routing.
There is no "re-balance" functionality implemented at the moment, but it can be done manually by "unstabilizing" and "re-stabilizing" with different collaterals for each party.

## How it works

When "stabilizing" part of the funds, the commitment transaction of the LN channel is replaced by a _split transaction_ with two outputs, one to keep funds within Lightning, and the other to fund a DLC channel.
That way both the LN channel and DLC channel can be updated independently.

The amount in the DLC channel is pegged to USD by setting up a CFD contract, represented by an hyperbola curve, with a collar at the outcome at which the maker's collateral becomes zero.

At the moment a single oracle is used, but the [underlying library](https://github.com/p2pderivatives/rust-dlc) can support multiple and thresholds of oracles (e.g. 2 of 3), though at the cost of a longer setup time.

More details about the construction can be found in [this article](https://medium.com/p/cb5d191f6e64).

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
Enter 1 for LN functions, 2 for wallet
>2
> getnewaddress
Receiving address: xxxxxxxx
Enter 1 for LN functions, 2 for wallet
>
```

Yet in another terminal:
```
bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest sendtoaddress xxxxxxxx 0.002
```
(using the address you generated previously)

Back to the first terminal:
```
Enter 1 for LN functions, 2 for wallet
>2
> getbalance
Balance: 200000
Enter 1 for LN functions, 2 for wallet
>1
To view available commands: "help".
> openchannel 02f7486899f42946a19d5f5bbdfb537d91e200dfda85408b014600a18255e53014@127.0.0.1:9001 180000
EVENT: initiated channel with peer 02f7486899f42946a19d5f5bbdfb537d91e200dfda85408b014600a18255e53014.
Enter 1 for LN functions, 2 for wallet
>
```
(replace the node id with that of the second instance)

Generate some blocks in another terminal:
```
bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest generatetoaddress 20 $(bitcoin-cli -rpcuser="testuser" -rpcpassword="lq6zequb-gYTdF2_ZEUtr8ywTXzLYtknzWU4nV8uVoo=" -regtest getnewaddress)
```

In the first terminal check that the channel is confirmed and do a keysend to the second node:
```
Enter 1 for LN functions, 2 for wallet
> 1
To view available commands: "help".
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
Enter 1 for LN functions, 2 for wallet
> 1
> keysend 02f7486899f42946a19d5f5bbdfb537d91e200dfda85408b014600a18255e53014 90000000
EVENT: initiated sending 90000000 msats to 02f7486899f42946a19d5f5bbdfb537d91e200dfda85408b014600a18255e53014
> Enter 1 for LN functions, 2 for wallet
>
EVENT: successfully sent payment of 90000000 millisatoshis (fee 0 msat) from payment hash "a5fd1b7544dc5794cb10869e5a7f676c68d37b472d27bfe028986275b05e67c5" with preimage "f9a7957e4946d082f5822586129d221691b34be5e2091d0e4e8c0993180621b9"
```

In the same terminal we can now "stabilize" part of the channel funds:
```
Enter 1 for LN functions, 2 for wallet
> 1
To view available commands: "help".
> stabilizechannel bc7121c4d2c809777930be4d82a944139fc815d010184ef20e4bfe72c20a17ca 45000 45000
Enter 1 for LN functions, 2 for wallet
>CFD contract setup done.
> 
```

It will take some time before the "CFD contract setup done." part appears, this is because the setup of the DLC requires signing and verifying a large number of adaptor signatures (using the parallel feature of rust-dlc can make it a bit faster).
Remember to use the proper channel id.

## License

Licensed under either:

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT License ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
