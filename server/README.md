# Online server list (`atv2-server`)

A small server that lists online races and relays them, so players join a
race from the launcher's list and nobody has to open a port on their router.
It's the `atv2-server` binary from the `atv-lobby` crate. It needs no game
files and uses one UDP port.

## Build

```
rustup target add x86_64-unknown-linux-musl
cargo build --release -p atv-lobby --bin atv2-server --target x86_64-unknown-linux-musl
```

This works on Windows too: `.cargo/config.toml` links it with Rust's own
`rust-lld`, so no Linux C toolchain is needed. The result,
`target/x86_64-unknown-linux-musl/release/atv2-server` (no `.exe`), is a
static Linux binary that runs anywhere, Pterodactyl's containers included.

## Pterodactyl

1. Admin > Nests: import `egg-atv2-server.json` (into a nest of your choice).
2. Create a server with the "ATV 2 Server List" egg. Its primary allocation
   is the port players use (any free port; 7777 is the game's default).
   Almost no memory or CPU is needed: 64 MB and a fraction of a core is plenty.
3. Upload `atv2-server` in the server's File Manager, then Start. The console
   says `atv2-server listening on UDP port N`, then each race going up and down.

Wings opens the allocation for both TCP and UDP. If the machine has its own
firewall (ufw, a cloud provider's security group), allow that UDP port too.

## Cloudflare

Add an A record (for example `atv`) pointing at the machine's IP with the
proxy turned **off** (grey cloud, "DNS only"). Cloudflare's proxy only
carries web traffic, so with the orange cloud on, the game's UDP packets
never reach the server. Players then use `atv.yourdomain.com:PORT` (the
port can be left off when it's 7777).

## Running it elsewhere

`atv2-server --port 7777` (or the `SERVER_PORT` environment variable). It
prints what it's doing and keeps nothing on disk.
