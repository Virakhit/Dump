# Local patch to libp2p-autonat 0.16.0

Source copied from the pinned crates.io release, upstream commit
`7171dce2f90c05ba7892d4ba926abb1881db27c7`. The upstream MIT license is in LICENSE.
No networking stack upgrade or wire format change.

Only the v2 client behaviour changes: completed candidates can be requeued or
removed without replacing active handlers; in-flight nonces remain intact;
closing the request connection releases its pending candidates; inconclusive
requests withdraw previous evidence and await the caller's backoff; confirmations require the exact candidate,
nonce, request connection and authenticated probe server. Scores saturate.
Dump bounds the cache at eight and schedules revalidation after five minutes.

Published integration-test declarations and their unused dev-dependencies were
omitted because their unpublished workspace-only test dependency is absent.
The client library regression tests
run with `cargo test --manifest-path src-tauri/Cargo.toml --locked -p libp2p-autonat --lib`.
Replace this patch when upstream exposes equivalent lifecycle-safe operations.
