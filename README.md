<h1 align="center">
  <img src="docs/klbf/logo.svg" alt="KLBF Lab" width="160">
  <br>
  KLBF Lab
</h1>

<h4 align="center">An open lab that combines the best fixes from every Xbox-to-PC compatibility project into one tested build.</h4>

<p align="center">
  <img src="https://img.shields.io/badge/current%20version-Lab%2023-ff5fa2" alt="current version: Lab 23">
  <img src="https://img.shields.io/badge/%F0%9F%A7%AA%20brewing-Lab%2024%20%26%2025-7c3aed" alt="brewing: Lab 24 and 25">
  <a href="https://github.com/Marvisak/VIctory"><img src="https://img.shields.io/badge/based%20on-VIctory-1f46c8" alt="based on VIctory"></a>
  <img src="https://img.shields.io/badge/tested%20on-AMD%20RX%209070%20XT-c04fb0" alt="tested on RX 9070 XT">
  <img src="https://img.shields.io/badge/AMD-first%20gameplay%20scene%20drawn-ffb000" alt="first gameplay scene drawn on AMD">
  <img src="https://img.shields.io/badge/license-GPL--3.0-2ea44f" alt="GPL-3.0">
</p>

<p align="center">
  <a href="#why">Why</a> •
  <a href="#status">Status</a> •
  <a href="#whats-inside">What's inside</a> •
  <a href="#build-and-run">Build and run</a> •
  <a href="#results">Results</a> •
  <a href="#testers-wanted">Testers wanted</a> •
  <a href="#rules">Rules</a> •
  <a href="#credits">Credits</a>
</p>

> [!IMPORTANT]
> This repository contains **no game files, keys, shaders or executables from the game**, and does not explain how to
> get them. You need your own legally obtained copy, already readable on disk. Nothing here decrypts or works around
> any protection. Everything a run creates stays on your PC: never share it.

## Why

Several open-source teams are working out, in public, how a modern console game can run natively on a PC: how its
graphics commands, shaders, memory and input get translated from one platform to another. It's fascinating to
watch, and with this many people following it, now is the time to learn how these systems really work.

**KLBF Lab is an educational project.** We read the code, build it, test it on real hardware and write down what we
learn. Every project solves different problems. So we **pick the fixes that really move things forward, wherever
they come from**, combine them in one build, measure it and credit the people who wrote them. We don't take sides:
our test numbers go back to every project.

### Why a fork, and not just a branch

We learn from the best projects first, VIctory above all, then XCL, Palmwave, and emulator write-ups like Xenia's and shadPS4's. But testing on one AMD card kept raising questions that no single upstream pull request answers: why the world stays black, why the GPU sits mostly idle, why one CPU thread is saturated. Answering them means trying things in a slightly different direction, measuring them, and sometimes throwing them away. That needed a place of its own, with its own build, its own test log and its own experiments, more than a branch in someone else's repository. Whatever works goes back upstream, with numbers.

**Every GPU.** Our own runs happen on an AMD card, but the goal is one build that works on every GPU: AMD, NVIDIA and Intel results are all welcome, and we try not to break any of them.

> [!NOTE]
> **Current version: Lab 23.** This branch is the [fxceless/victory-gta6](https://github.com/fxceless/victory-gta6) fork, unchanged, plus our docs. All rendering work here is by its author (see [README.fxceless.md](README.fxceless.md)). We tested it on AMD: it draws the first gameplay scene on an RX 9070 XT. Our own fixes go on top in the next builds; the previous KLBF stack is kept as the tag `klbf-lab-15`.
>
> 🧪 **Brewing in the lab:** Lab 24 (stop the memory growth so a run lasts) and Lab 25 (our shader fixes on top, for more fps). They replace this version once one passes a full run.

## Status

**Not playable.** Measured on our machine (Windows 11, AMD Radeon RX 9070 XT, Ryzen 7 9800X3D, 32 GB RAM):

| Stage | KLBF Lab (this branch), on our RX 9070 XT |
|---|---|
| Boot, first-boot screens, intro video, start screen | 30 fps |
| Loading into the world (~50 s) | works |
| The opening scene | plays |
| The first gameplay scene | **drawn**: characters, buildings, the yard (seen with the game's high-contrast option on) |
| Speed in the world | ~2 fps after warm-up (~520–550 ms/frame) |
| Stability | no device removal; **the run stopped at 514 s when Windows ran out of virtual memory** (the process committed ~104 GB) |

**Known problem on our machine:** committed memory keeps growing (the fork's author reports ~24 GB on an RTX 4070). Close other programs, keep a large page file, and expect a stop after several minutes. We're working on it.

## Features (Lab 23)

What we checked ourselves on the RX 9070 XT, and what the fork's author reports. "Not tested" means we have no evidence either way yet.

### Controls

| Feature | Status | Notes |
|---|---|---|
| Xbox controller (XInput) | Works | Menus and gameplay input, vibration; checked on our earlier builds, not yet re-checked on Lab 23 |
| Controller plugged in after launch | Not yet | The game only sees controllers connected before it starts: plug in first, then launch |
| Keyboard and mouse for gameplay | Not yet | Planned (VIctory #1529); today the game asks for a controller |
| Sony DualSense (PS5) controller | Not yet | Planned (VIctory #1517) |
| Scripted key presses for testing (`VICTORY_KEYS`) | Works | Used by our test script |

### Display

| Feature | Status | Notes |
|---|---|---|
| Windowed mode | Works | Resizable; can be moved to any monitor |
| Fullscreen (Alt+Enter) | Not yet | Freezes the picture in the world; stay windowed |
| Ultrawide and multi-monitor desktops | Works | Run on a 5120×1440 + 2560×1440 setup, window on either screen |
| Frame rate in menus | Works | 30 fps, the game's own cap |
| Frame rate in the world | Partial | ~2 fps after warm-up |
| First gameplay scene drawn | Partial | Characters, buildings and the yard; areas in shadow stay dark (ray-traced lighting not implemented) |
| HDR | Not tested | |

### Game

| Feature | Status | Notes |
|---|---|---|
| Boot, legal and first-boot screens | Works | |
| Intro video with sound | Works | |
| Game menus in French | Works | Other languages not tested by us |
| Accessibility: high-contrast option | Works | Makes the scene easiest to read today |
| Opening scene, audio and subtitles | Works | |
| Mission objectives | Works | Shown on screen |
| Pause menu, HUD | Reported | By the fork's author on an RTX 4070; not checked by us yet |
| Autosave and loading a checkpoint | Reported | By the fork's author; not checked by us yet |
| Long sessions | Partial | Our run stopped at 8.5 min (memory growth); Lab 24 targets this |

## What's inside

- The [fxceless/victory-gta6](https://github.com/fxceless/victory-gta6) fork at `578d459` (VIctory `main` `ffe310e` + one commit with its rendering, shader and speed fixes), unchanged. All credit to its author.
- Our docs: this README, [docs/klbf/results.md](docs/klbf/results.md), [docs/klbf/run-test.ps1](docs/klbf/run-test.ps1).

Patch notes for every version: [CHANGELOG.md](CHANGELOG.md). Earlier KLBF versions (older, kept for reference) are tags:
- `klbf-lab-15`: the AnyXboxX integration + #431 + #67 (25 min with no device removal, ~1 fps, dark world).
- `klbf-lab-03`: VIctory `main` + #452, #431, #67, #584, #493 (1,006 s, black world).

## Build and run

Same requirements as VIctory: Windows 10/11 x64, an AVX2 CPU, a Direct3D 12 GPU with shader model 6.6 and mesh
shaders, Rust. See [README.fxceless.md](README.fxceless.md) (it includes the original VIctory README) for the full setup (`victory.toml`, paths, controls).

```powershell
git clone -b klbf-lab https://github.com/PastiiJay/klbf-lab
cd klbf-lab
copy victory.example.toml victory.toml        # then set your paths (outside the repo)
cargo build --release -p victory --features diagnostics
$env:VICTORY_D3D12_DRED = '1'                 # clearly more stable on our card
target\release\victory.exe run <game dir>\<the title's exe> --execute
```

Tips from our runs:
- **Stay windowed.** Alt+Enter (fullscreen) freezes the picture in the world.
- `docs/klbf/run-test.ps1` runs a timed test and logs the caption (fps, draws per frame) every 15 s.
- When the GPU is removed, Windows resets the display driver: the screen and mouse freeze for a moment. That's expected.

## Results

Every run we made, with build and result: **[docs/klbf/results.md](docs/klbf/results.md)**.
The same numbers are posted to the projects' own threads ([VIctory #59](https://github.com/Marvisak/VIctory/issues/59),
[XCL #560](https://github.com/ivall/xcl/issues/560), [Palmwave #2](https://github.com/fablinoo/palmwave/issues/2)).

## Testers wanted

We're looking for **testers**, not chatters. Any GPU: AMD, NVIDIA or Intel.

1. Build the `klbf-lab` branch from source, with your own copy of the game.
2. Run it with `docs/klbf/run-test.ps1`.
3. [Open an issue](https://github.com/PastiiJay/klbf-lab/issues/new) with your GPU, driver, build commit, how many seconds it ran and the last error lines of the log. Text only.

> [!NOTE]
> **We have no game files and will never share any. Please don't ask.**
> **There is no Discord.** Everything happens here on GitHub, in issues and pull requests. Any server claiming to be KLBF Lab is not us.

## Rules

- **No game content.** No game files, extracted shaders, saves, keys, screenshots or videos in this repository, in issues or in pull requests.
- **No ready-made binaries.** Build it yourself from source.
- **Credit first.** Every fix keeps its author and links the original pull request.
- **Facts, not hype.** A claim comes with a build, a GPU and a log line.

## Credits

All the real work belongs to the authors of [VIctory](https://github.com/Marvisak/VIctory) and of the pull requests
listed above, and to the [XCL](https://github.com/ivall/xcl) and [Palmwave](https://github.com/fablinoo/palmwave)
teams. KLBF Lab only selects, combines and measures.

## License

[GPL-3.0](LICENSE), the license of VIctory. The fork's README (with the original VIctory README below it): [README.fxceless.md](README.fxceless.md).
