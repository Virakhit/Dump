# Local patch to libp2p-stream 0.5.0-alpha

Source copied from the pinned crates.io release, upstream commit
`7171dce2f90c05ba7892d4ba926abb1881db27c7`. The original upstream MIT license
(Copyright 2017-2020 Parity Technologies (UK) Ltd.) is preserved in LICENSE.
The only runtime change is removing the closed connection sender in
`Shared::on_connection_closed`. Upstream retained that map entry indefinitely;
an Internet node with connection churn otherwise accumulates handler channels.
The appended regression test exercises 4096 open/close cycles.
No wire protocol or public API changes. Replace this local patch when upstream
provides equivalent cleanup. Keep Cargo.lock and run the package library test.
