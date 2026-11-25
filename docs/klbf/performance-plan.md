# Performance plan (branch `amd-optimized`)

Machine: RX 7900 XTX 24 GB, Ryzen 7 5800X3D, 64 GB. Measuring scene: the prologue's
cell, Lucia standing still, the last 60 s of a 330 s run.

## Starting point

| | base (W0, Z0) | today (AD1) |
|---|---|---|
| frame | 213-218 ms | 167 ms (6 fps) |
| submitting thread, preparing | 125 ms | 115 ms |
| of that, copying memory | 75 ms | 73 ms |
| GPU | 77 ms | 35 ms |
| read-back thread (in parallel) | 104 ms | 94 ms |

AD1 is a single run (AD0, cold: 180 ms). The limit is the CPU, on the submitting
thread. AD1's numbers per frame, summed over threads: 13,554 title copies for only
6.6 MB (35 ms of kernel calls); 2,447 "what changed?" askings over 9.0 GB, 98 %
unchanged (69 ms); 107 ms of the read-back thread, 40 of them waiting for its own
copies.

## Rules for each stage

1. One stage per commit, with a switch that turns it off, on `amd-optimized`.
2. Before: `cargo test` of formats, shader, gpu and hle. The `watch` tests fail now and
   then from before (shared state): run those alone and in series.
3. Measure with the same executable, interleaving with/without the switch: one cold
   run that does not count and two pairs. Accepted if the median drops 5 ms or more in
   both pairs. Runs normally differ by 15-30 ms.
4. In every run: 0 device losses, 0 `request failed`, 0 exits, and a capture against U
   (suit, mesh, walls, objects, eyes, hoodie in the cutscene).
5. If something breaks, turn the switch off and go back to the commit before
   (`prueba-u`, `prueba-ac`). Publish only with permission.

## Stages

### 1. What was already brought in (branch `lab-s`, being measured as AE)
- 256 lists in flight, a copy queue of its own, locks per range, texture
  detiling (#603, #993), depth bounds test, textures before composition, one upload
  per texture.
- AE0-AE4 measure it all together against AD. If it is better or equal, it goes to
  `amd-optimized` by cherry-pick (5 commits, no conflicts with what is there). If it
  is worse, it is taken apart with `VICTORY_LATE_WAIT_LISTS=128`,
  `VICTORY_RELAY_COPY_QUEUE=0`, `VICTORY_MAPPING_RANGES=0`,
  `VICTORY_EARLY_TEXTURES_MS=0`, `VICTORY_BAKE_TEXTURES_MS=0`.
- Risk: the copy queue reads memory other work may be writing; look for flicker or
  old data.
- Also: make the switches that already won the default
  (`VICTORY_INDIRECT_WINDOW`, `VICTORY_WIDE_REFRESH`, `VICTORY_WIDE_MERGE`) and a
  `play-amd.ps1` launcher without DRED.
- Effort: low. Expected: 10-25 ms standing still; more while moving.

### 2. Copies that do not go through the kernel (done, accepted)
- `usercopy.rs`: a routine of our own copies in place; if a page is not there, a
  handler sends it to its failing return and the kernel's way is taken. For any size,
  both ways. `VICTORY_USER_COPY=0` turns it off.
- AF1/AF3 with it: 165/167 ms. AF2/AF4 without: 173/181 ms.

### 2b. Shared memory, partial (measured as AG, kept as a candidate)
- `VICTORY_INPLACE=1`: the title's private memory is opened as the card's memory and
  no longer copied. It was already in the base, off. Level 2 does not go with
  submissions sent ahead.
- AG1/AG2: 162/161 ms against 165/167 (AF1/AF3), preparing 8 ms shorter, 384 MB
  opened and some 1,100 ranges turned down. Under the 5 ms rule in one of the pairs,
  and not interleaved: measured again once the rest is in.
- Risk: the title cannot free or protect that memory while it is open.

### 2c. Idle textures swept by parts (an idea of xcl's, its PR 490)
- `resource.rs` `let_go_idle` goes through some 67,000 textures in every sweep and
  costs the submitting thread about 3 % (profile P0). Only a part is looked at each
  time.
- Effort: low. Expected: 3-5 ms. Written in branch `etapa-3`, not measured.

### 3. Ask less what changed, and copy less
- 9.0 GB looked through per frame to find 0.18 GB. A 174 MB block is looked through
  some 14 times per frame.
- How:
  a. **4 KiB pages (an idea of xcl's).** Windows reports writes by 4 KiB pages, but our
     journal rounds them to 64 KiB and copies all 64. Measure first: how many of the
     ~190 MB copied per frame really changed. Written in branch `etapa-3`
     (`VICTORY_DIFF_COPY`), not measured.
  b. **Fingerprint per page sent (an idea of xcl's).** A page rewritten with what it
     held is not sent to the card again.
  c. Memory watched by protection: a stamp per region that rises whenever a block is
     opened or a mapping changes. A range without new stamps is answered without
     asking. The ordered sets (`clean`, `protected`) become bitmaps.
  d. One asking per region and submission, and no `VirtualQuery` on the hot path: xcl
     measured 75-80 ms per call in a 4 GB reservation, with the address space locked
     for everyone.
- Files: `watch.rs`, `shadow.rs` (`update_free`, `merge_under_writes`).
- Risk: a lost notice leaves old data. Check mode: a switch that asks anyway and
  counts differences; it must give 0 before it is turned on.
- Effort: medium. Expected: 15-30 ms, or more if (a) pays.

### 4. Shaders that mark what they write (VIctory `cd94d35` + `940d32f`)
- What it takes out: the passes that hash and compare ~2,000 pieces per frame on the
  card, fetching ~30-70 MB and comparing them on the CPU.
- Dry run: conflicts in 17 files, and 5 test files do not exist in our base. It is not
  taken whole; it is ported in three steps, each of which compiles:
  1. Translator (`victory-shader`): option `view_marks`, a bitmap per 64 KiB page. Off
     by default; the ~25,000 shaders are validated with the scan.
  2. GPU: a table per slot and a bitmap (`descriptor/host.rs`, `draw.rs`,
     `pipeline/host.rs`, `shader_cache.rs`). Only counting at first: the marks against
     what today's comparison finds. The marks must cover all of it.
  3. Read-back: in `Host::card_before` and `relay_card_back`, the marks instead of the
     hashes. `VICTORY_VIEW_MARKS=0` goes back to today's.
- It also solves the 174 MB block of the 7 shaders at its root (today it is
  approximated by learning where they write).
- Risk: high; everything is translated again (a slow first run) and code is added to
  every write. A branch of its own from `amd-optimized`.
- Effort: high. Expected: 20-40 ms of the read-back thread and some GPU. The frame only
  shows it if that thread is the limit, so it comes after 2 and 3.

### 5. AMD FSR (klbf-lab `ffx.rs`)
- Only if the GPU becomes the limit after 1-4 (today 35 of 167 ms). New files, but it
  needs the FidelityFX library, loaded from a folder.
- Effort: medium. Expected today: almost nothing.

### 6. A single mirror of the title's memory (an idea of xcl's, long term)
- xcl does not have thousands of copies: it mirrors the title's addresses in a few 4 GB
  windows of VRAM (tiled resources), asks what changed once per submission for all, and
  sends only the pages touched through a 64 MB ring that does not wait. We have ~4,500
  copies and 829 windows that overlap.
- It means doing `shadow.rs`, the shaders that read by address and the read-back over.
  Only if copies are still the limit after 1-4. Effort: very high.
- **Ideas only:** xcl is GPL-2.0-only and this is GPL-3.0-or-later. No code is copied.

## Order and decision points

1 → 2 → 2b → profile → 2c → 3 (a first) → profile → 4 → 5 → 6.

- After 1: if AE is not better, decide by pairs which pieces stay.
- After 3: with a new profile. If the limit is still the submitting thread, go on there;
  if it moved to the read-back thread, do 4.
- A reasonable goal for 1 to 3, if the gains add up: 110-130 ms (8-9 fps) standing
  still. It is an estimate, not a promise.

## Pending outside this plan
Full shader validation (missing since R), clothes that come and go in the cutscene,
remains of earlier shots at the bottom, bounced light and ray tracing.
