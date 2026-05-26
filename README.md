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
| **Orca Whirlpools** | ✅ Read + write paths wired against `orca_whirlpools` 8.0.0 and verified against live devnet: `fetch_state` (price, range, real inventory, fees), plus `ensure_position` and `recenter` (close+open) building and signing real transactions, gated by `dry_run`. Build with `--features orca`. |
| **Raydium CLMM** | 🔧 Wiring template. Trait implemented; on-chain calls are marked TODO. Build with `--features raydium`. |

### Building the Orca path

The default build is the **simulator** — no heavy SDK, compiles fast:

```
cargo build                   # simulator
cargo build --features orca   # real Orca path
```

The crate is on the **Solana v3** split crates throughout (`solana-pubkey`,
`solana-keypair`, `solana-signer`), which is what `orca_whirlpools` 8.0.0
targets — so `--features orca` is version-consistent. Note v3 removed
`Keypair::from_bytes`; the code uses `Keypair::try_from(&[u8])`. If cargo reports a transitive `solana-program`/`anchor` pin conflict on first
build, apply the lockfile patch from the Orca docs (`cargo update
solana-program:<cur> --precise <req>`).

---

## Running it against Orca (devnet → mainnet)

The simulator needs no setup. To run the **real Orca venue**, you configure
three things in `config.toml`: a **wallet**, the **token pair** to make markets
in, and **deposit amounts**. There is no separate wallet store or UI — the bot
reads everything from `config.toml` at startup.

### 0. Prerequisites (macOS or Windows)

The bot is pure Rust and runs identically on macOS (Apple Silicon or Intel),
Linux, and Windows. You need Rust (`rustup`) and, for creating/funding wallets,
the Solana CLI.

Rust, if you don't have it: https://rustup.rs — on macOS that's
`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`.

### 1. Install the Solana CLI (for creating/funding wallets)

**macOS / Linux** (zsh/bash):

```bash
sh -c "$(curl -sSfL https://release.anza.xyz/stable/install)"
# the installer prints a line to add to PATH; or for this session:
export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"
solana-keygen --version
```

To make it permanent, add that `export PATH=...` line to `~/.zshrc`.

**Windows** (PowerShell):

```powershell
cmd /c "curl https://release.anza.xyz/stable/solana-install-init-x86_64-pc-windows-msvc.exe --output C:\solana-install-tmp\solana-install-init.exe --create-dirs"
C:\solana-install-tmp\solana-install-init.exe stable
# restart the shell, or add to PATH for this session:
$env:Path += ";$env:USERPROFILE\.local\share\solana\install\active_release\bin"
solana-keygen --version
```

### 2. Create and fund a wallet

Use a **throwaway keypair** for devnet, never a real key. The `solana-keygen`
commands are identical on every platform:

```bash
solana-keygen new -o devnet-throwaway.json --no-bip39-passphrase
```

Then airdrop. **macOS / Linux:**

```bash
solana airdrop 2 "$(solana-keygen pubkey devnet-throwaway.json)" --url devnet
```

**Windows** (PowerShell uses `(...)` instead of `"$(...)"`):

```powershell
solana airdrop 2 (solana-keygen pubkey devnet-throwaway.json) --url devnet
```

**The wallet must hold the tokens it will deposit.** A concentrated-liquidity
position deposits *both* tokens of the pair (or one, depending on where the
price sits relative to your range). An airdrop gives you SOL only — if your pair
is SOL/USDC you also need some USDC in the wallet, or the open will fail with an
insufficient-funds error at send time. (On devnet, getting the quote token means
swapping or minting it; on mainnet you fund the wallet with both real tokens.)


### 3. Configure `config.toml`

Start from the devnet template (macOS/Linux use `cp`, Windows uses `copy`):

```bash
cp config.devnet.toml config.toml      # macOS / Linux
# copy config.devnet.toml config.toml  # Windows
```

The lines that matter:

```toml
venue        = "orca"
rpc_url      = "https://api.devnet.solana.com"
orca_network = "devnet"                 # MUST match the rpc_url's cluster

wallet_path  = "devnet-throwaway.json"  # THE WALLET — path to the keypair file

# THE PAIR being market-made — the two token mints + tick spacing identify the pool:
orca_token_a      = "So11111111111111111111111111111111111111112"  # e.g. wSOL
orca_token_b      = "BRjpCHtyQLNCo8gqRUr8jtdAj5AjPYQaoqbvcZiHok1k"  # e.g. devUSDC
orca_tick_spacing = 64
orca_decimals_a   = 9
orca_decimals_b   = 6

# DEPOSIT amounts (raw base units). These tokens MUST be in the wallet.
orca_deposit_max_a = 0
orca_deposit_max_b = 1000000            # 1 devUSDC (6 decimals)

dry_run = true                          # keep TRUE until you've watched a clean run
```

To market-make a **different pair**, change `orca_token_a`/`orca_token_b`, the
matching `tick_spacing`, the decimals, and the deposit amounts — and make sure
the wallet holds those tokens.

### 4. Dry-run first (sends nothing)

```powershell
cargo run --features orca
# open http://127.0.0.1:8787 and click Start
```

(On macOS, the first run may pop a firewall prompt because the bot opens a local
port — click Allow. It only ever binds `127.0.0.1`, so it stays on your machine.)

With `dry_run = true`, the bot reads live pool state (you'll see a real price and
inventory on the dashboard) and **logs the transactions it would send without
sending them**, e.g.:

```
Orca ensure_position: dry_run, NOT sending 6 ix (mint would be Emp...)
```

That confirms the read path talks to the live cluster and the write path builds
valid transactions. Watch a few ticks before going further.

### 5. Go live (sends real transactions)

Only after a clean dry-run, and only once the wallet holds the deposit tokens:
set `dry_run = false` and re-run. Now Start/Recenter actually open and rebalance
positions on-chain.

**Order of operations, always:** devnet with a throwaway wallet first → confirm a
real open + recenter works end to end → only then point `rpc_url`/`orca_network`
at mainnet with a real wallet and real funds. Every recenter realizes impermanent
loss and pays fees (see the warning below); start with small deposits.

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
