# sol-clmm-mm

A **self-hosted concentrated-liquidity market maker** for Solana — Orca Whirlpools
or Raydium CLMM, behind one trait — with a **local web dashboard** for live
control. Runs out of the box against a built-in simulator: no SDK, no RPC, no
funds required to see the whole thing work.

```
cargo run            # uses ./config.toml (venue = "simulated")
# open http://127.0.0.1:8787
```

---

## What this is (and what it isn't)

**It is:** a real rebalance engine + a real local control panel. The loop reads
pool price, checks whether the live price still sits inside your position's
range, and recenters (close + reopen around price) when it drifts past a
configurable buffer. Start/stop/recenter live from the dashboard; live PnL,
inventory, fee accrual, and a rolling action log stream over a localhost
WebSocket.

**Honest status of the venue integrations:**

| Venue | Status |
|---|---|
| **Simulated** | ✅ Fully working. Real price walk, real position, real IL realized on recenter, real (configurable) fees. The entire bot + UI runs against this today. |
| **Orca Whirlpools** | 🔧 Wiring template. Trait is implemented; each on-chain call is a clearly-marked `todo!()` with notes on which SDK call goes where. |
| **Raydium CLMM** | 🔧 Wiring template. Same as Orca. |

The Orca/Raydium files are **not compile-tested against the real SDKs** and were
deliberately not written from memory — the SDK 7.x surface shifts between point
releases, so guessed calls would compile-fail in subtle ways. You fill the
`todo!()` bodies against the actual crate docs for the version in your
`Cargo.lock` (or paste from your existing v0.2). Everything *around* those calls
— the engine contract, state, dashboard, safety — is final.

---

## Why the UI is local, not GitHub Pages

A CLMM market maker is a persistent process that holds your wallet key, keeps an
RPC connection open, and runs a rebalance loop. GitHub Pages is static hosting —
it can't run a binary, hold a key, or keep a loop alive. So **GitHub holds the
source; the dashboard runs locally** on `127.0.0.1` (it never binds `0.0.0.0`).
Your key never leaves your machine. This is the secure design, not a compromise.

If you ever want a public GitHub Pages control panel, it can only be a thin
client that connects back to a bot you're still running locally — more moving
parts (HTTPS-page → localhost mixed-content/CORS) for a cosmetic gain. Not
included by default for that reason.

---

## Setup

```bash
cp config.example.toml config.toml
cargo run
```

`config.toml` is gitignored. For a real venue, set `venue`, `rpc_url`, `pool`,
and `wallet_path` (a Solana CLI keypair JSON — also gitignored).

### Key config knobs

- `half_width_bps` — range half-width. `250` = ±2.5% around price.
- `rebalance_trigger_bps` — how far price must drift *past* a range edge before
  recentering. A buffer that stops boundary thrashing.
- `rebalance_cooldown_secs` — minimum time between recenters regardless of price.
- `dry_run` — **defaults to `true`**. No transactions are sent; intended actions
  are logged. Flip to `false` only after wiring a real venue and testing on devnet.

---

## ⚠️ Read before using real funds

- **Every recenter realizes impermanent loss and pays fees.** A recenter is
  withdraw + re-add liquidity. Too tight a range or too sensitive a trigger will
  churn your position and bleed fees. The defaults are conservative on purpose;
  the trigger buffer + cooldown exist specifically to limit this. Tune
  deliberately, watch the simulator first.
- **`dry_run = true` by default** so a first `cargo run` can't touch mainnet.
- **Test on devnet** before mainnet, with a throwaway keypair.
- This is infrastructure, not financial advice. You own the strategy and the risk.

---

## Wiring a real venue

Open `src/venues/orca.rs` or `src/venues/raydium.rs`. Each method has a comment
block describing exactly what the on-chain calls must do:

- `fetch_state` — read pool + position accounts, convert sqrt-price/ticks to a
  `PoolState`.
- `ensure_position` — open an initial position if none exists (honor `dry_run`).
- `recenter` — decrease liquidity to zero + close old position, open + fund a new
  one centered on price; return realized PnL, cost, and tx signatures.

Add the venue SDK crate to `Cargo.toml` (pin the exact version you verify), fill
the `todo!()` bodies, and the existing engine + dashboard drive it unchanged.

## Layout

```
src/
  main.rs            entry: load config, build venue, spawn engine + web
  config.rs          config.toml + keypair loading (key never logged)
  venues/
    mod.rs           Venue trait + PoolState + factory
    simulated.rs     fully working simulator
    orca.rs          Orca Whirlpools wiring template
    raydium.rs       Raydium CLMM wiring template
  engine/
    mod.rs           rebalance loop + recenter decision
    state.rs         shared snapshot + command channel
  web/
    mod.rs           axum server (127.0.0.1) + websocket
    dashboard.html   embedded control panel
```
