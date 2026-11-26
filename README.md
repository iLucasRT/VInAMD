# VInAMD

A fork of [VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX), itself a fork of
[VIctory](https://github.com/Marvisak/VIctory): a compatibility layer that runs an Xbox Series title
natively on Windows from files the user supplies. This one is about **playing on an AMD Radeon
RX 7900 XTX**: speed first, without crashes, then the picture.

**This repository contains no game files, code or assets.** You need your own legally obtained copy
of the game. It is not affiliated with or endorsed by the game's developer or publisher.

## Where it stands (2026-11-26)

Tested on one machine only: Windows 11, AMD Ryzen 7 5800X3D, 64 GB RAM, Radeon RX 7900 XTX 24 GB,
diagnostics build, `scripts\play.ps1`'s settings, shaders translated and pipelines made ahead.

| | |
|---|---|
| Menus, videos, loading screens | 30 fps |
| Open world and city, played | median 80 ms a frame (12-13 fps); one frame in ten over 101 ms |
| Stutters | 27 % of seconds have a frame over 150 ms, 3 % over 250 ms |
| Looking at the ground (few draws) | about 43 ms a frame (21-23 fps) |
| Longest session played | 32 minutes; 17 minutes with these settings, no device loss |
| Memory | about 40 GB of private memory; video memory held near 80 % of its budget: **64 GB of RAM needed** |

The window's caption tells where a frame's time goes: peak frame, CPU and copies, copying back,
the card's busy share, video memory, RAM, free commit, and pipelines and textures pending
(`VICTORY_TITLE_STATS=0`: the frame rate only). A frame over 150 ms gets a line of its own in the
log saying where its time went (`victory_gpu::slow`, `VICTORY_SLOW_FRAME_MS`).

What is known to be wrong:

- **Stutters.** What costs most in a slow frame: preparing the submission on the CPU (~70 ms), the
  texture side of the pass over the view heap (~55 ms) and looking at its slots (~33 ms). Renderer
  port 2 would remove the last, but on this machine it leaves the indirect light black (it misses
  thousands of the title's view writes a frame); it stays off.
- **Things that come into view** can stay black or at their lowest detail for some seconds.
- The first run of a new build or driver is slow until its pipelines are made: run
  `victory.exe warm --used` after a session (it makes the ones played with).
- Hair can be drawn over faces in some shots.

## What is different here

On top of VIctory-7900XTX (merged up to c9c932c; what that fork does, and why, is in
[docs/findings.md](docs/findings.md)):

- **Raw accesses to buffers picked from the view heap stay inside the view found there**
  (`VICTORY_PICK_BOUND`, on). On AMD a raw access through a structured or typed view is held to the
  element index alone, and a byte offset read from memory that holds something else reached up to
  4 GiB past the view; Radeon GPU Detective named the load. On this machine, without the bound every
  run that moved through the world lost the device within 4 to 10 minutes; with it none did.
- **Video memory kept below its budget:** textures are let go from just under the share where new
  ones stop being made (`VICTORY_TEXTURE_ROOM_PERCENT`, 80 in `play.ps1`); at upstream's 88 %, the
  card filled after some 17 minutes and spilled into the PC's memory (130-147 ms of GPU a frame).
- **The PC's memory:** host copies of textures and empty blocks of the card's memory go when the PC
  has little left to commit (`VICTORY_TEXTURE_COMMIT_MB`, 2048); staging blocks keep their spares.
- **Card-awake off in `play.ps1`:** with it, the driver reset twice after 10-12 minutes with only its
  dispatch in flight.
- **Pull requests of VIctory-7900XTX taken early:** #92 (TheHoracio), #98 (Joly0), #105 and #108
  (SousaTrashBin), #96 (kalijerry; its part for capped heaps left out), #126 (ESAT586) and #141
  (mattkajorinc: thin LTO). #24, #77, #87 and #128, taken earlier, are in upstream's main now; #68
  and #125 were left out where upstream reworked the same code (renderer port 6, pipeline waits).
- **The caption's performance figures**, the slow-frame line, and stopwatches of the texture side of
  the pass over the view heap (diagnostics).

## How to run it

You need Windows 10/11 x64, a CPU with AVX2, a Direct3D 12 GPU with shader model 6.6 and mesh
shaders, Rust (the toolchain is pinned), the MSVC build tools, and your own dump of the title.

```powershell
git clone https://github.com/iLucasRT/VInAMD
cd VInAMD
copy victory.example.toml victory.toml      # then set your paths, outside this folder
cargo build --locked --profile diag -p victory --features diagnostics
target\diag\victory.exe shaders             # every shader translated ahead, about 10 minutes
scripts\play.ps1 -Exe <path of your copy's executable>
target\diag\victory.exe warm --used         # after a first session: its pipelines made ahead
```

`scripts\play.ps1 -Safe` leaves out the newest, faster ways; try it first when something is wrong.
`-RayTracing` turns the title's ray-traced indirect light on, about a third slower. Stay windowed.

Commands, debugging switches and upstream's own description: [docs/README-fxceless.md](docs/README-fxceless.md).
Every `VICTORY_...` switch is described in a comment where it is read.

## Reporting a test

Open an issue with text only: GPU and its memory, driver, CPU, RAM, page file size, the commit
(`git rev-parse --short HEAD`), how long it ran and how far you got, the caption's figures in a menu,
indoors and in the open world, and the last lines of the log (check that they hold no personal path
or name). **Never attach** game files, shaders, saves, screenshots, videos or captures.

## Credits

This is a tree of other people's work with a little on top. The history is kept, and commits taken
from elsewhere name the commit and its author.

- **[VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)** by avatarmanz: the base of
  this fork, and most of its speed on RDNA 3.
- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its contributors: the loader,
  the runtime services, the Direct3D 12 back end and the shader recompiler.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)**: the fork VIctory-7900XTX
  started from, and the people whose pull requests it carries.
- **[DexNom/VIctory](https://github.com/DexNom/VIctory)**,
  **[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)** (with
  romainhedouin), **[FQY55kai/ZJOU-VICTORY](https://github.com/FQY55kai/ZJOU-VICTORY)**,
  **[myokex/victory-amd](https://github.com/myokex/victory-amd)** (with Spexivus, joroyt, tightfall
  and Joshua Odom), **Geckomacabre** and **cxy0072019-sketch**: as credited by VIctory-7900XTX.
- **TG0000**: the guard page that stays one (VIctory 79c9b93).
- **[KLBF Lab](https://github.com/PastiiJay/klbf-lab)** by PastiiJay: where this fork's own work
  began; that earlier work is kept in this repository's other branches.
- **[XCL](https://github.com/ivall/xcl)** (GPL-2.0-only) was read for how the same title behaves
  under another layer; no code of it is here.
- Changes of this fork and of VIctory-7900XTX were made with [Claude Code](https://claude.com/claude-code).

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE); the one file that is not covered is named in
[docs/asset-policy.md](docs/asset-policy.md).
