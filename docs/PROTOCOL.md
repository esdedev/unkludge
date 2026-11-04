# Unkludge protocol notes

How Unkludge talks to the Royal Kludge keyboards it supports: the 29 models with `fw = 1` and a screen in `data/models/`, which `unkludge models` lists.

## Where this comes from, and how far to trust it

These notes describe what the official RK app does, worked out by static analysis of that app and its config files, for interoperability. Unkludge follows it.

**No command here has been confirmed by a USB capture yet.** Making that capture is a requirement for the v0.1 release, and the table below tracks it. Until then, read everything here as "what the official app's code says", not as "what the firmware needs". What's known only as a guess is listed under [Not confirmed](#not-confirmed), not in the sections before it.

## Commands and their status

| Command | What it does | Data | Answer | Seen in our own capture | Observed on S85 ES (`0265`) |
|---|---|---|---|---|---|
| `0x84` | Read the config block | 128 bytes | Feature report | no (pending) | Yes: with Unkludge on 2026-11-03 (`info`, `light`) |
| `0x04` | Write the config block | 128 bytes | None | no (pending) | No |
| `0x0A` | Write the palettes | 420 bytes | None | no (pending) | No |
| `0x0B` | Set the clock | 14 bytes | None | no (pending) | No |
| `0x0C` | One chunk of screen data | Up to 512 bytes | Ack | no (pending) | No |
| `0x0D` | Start or finish a screen upload | 5 bytes | Ack | no (pending) | No |
| `0x0D` | Abort a screen upload | 5 bytes | Ack | no (pending) | No |
| `0x82` | Read the 6-byte identity, to pair a second collection | None | Feature report | no (pending): needs an N99 owner | No: only `kind = 2` models use it |

What "observed" means here:

- **Unkludge, 2026-11-03:** two read-only commands, `unkludge info` and `unkludge light`. The keyboard answered with a block ending in `5A A5`, and the values matched an earlier read. That is the only command Unkludge has ever sent to a keyboard. Everything else it can do has only run against a simulated keyboard.

## Transport

- USB HID, vendor ID `258A`, wired only.
- Which collection: usage page `0xFF02`, usage 2. `kind = 2` models also have a second collection, usage page `0xFF03`, usage 2.
- On Linux, when the system doesn't list usages, Unkludge takes interface 1, the one that holds this collection on the S85.
- Everything sent is a **feature report of 520 bytes** including the report ID, which is always `0x09`. Whatever a report doesn't use is zero.
- Acks arrive as 8-byte input reports.
- The system lets several programs open a collection at once, and what a keyboard does when their reports mix isn't known. So Unkludge's own programs take a lock before opening a keyboard, reads included, and keep it until they close it: a named mutex on Windows, `flock` on a file on Linux. Both go away with the process, however it ends. This is Unkludge's own rule, and the keyboard knows nothing of it.

## Report layout

```
09 <cmd> <arg> 00 <chunks> <idx> <len lo> <len hi> <up to 512 data bytes>
```

`chunks` is how many reports the data takes, `idx` which of them this is, and `len` the data bytes in this report. Screen chunks differ, as described under [Screen](#screen).

A read sends `09 <cmd> <arg> 00 01 00 <len lo> <len hi>` with no data, where `cmd` has its top bit set. The answer is fetched as a feature report and repeats that header, followed by the data.

## Pacing

| Step | Wait |
|---|---|
| Before each report of a command with no ack, each read request and each `0x0D` | 25 ms |
| Between a read request and fetching its answer | 25 ms |
| A report that fails to send | Up to 3 tries, 50 ms apart, or 100 ms for `0x0C`. On reports with an ack, the ack wait still decides after the third |
| Before the config read and the config write when changing settings | 30 ms each |
| Between the config write and the palettes | 100 ms |
| Between `0x0C` chunks | None: the ack sets the pace |

## Config block

128 bytes, read with `0x84` and written with `0x04` as a whole.

```
read:  09 84 00 00 01 00 80 00
write: 09 04 00 00 01 00 80 00 <128 bytes>
```

| Offset | Meaning |
|---|---|
| `0x09` | 1 when the custom mode is on |
| `0x0A` | Effect number, 0 for off |
| `0x18` | Sleep time |
| `0x38 + 2 * effect` | That effect's brightness byte |
| `0x39 + 2 * effect` | That effect's `speed << 4 \| colour`. Colour 7 means rainbow, 0 to 6 a palette slot |
| `0x7E`, `0x7F` | `5A A5`, the end marker |

- A read is tried up to 5 times, 100 ms apart, and must end in the marker. Without it, nothing is written.
- Unkludge writes back what it read, with only the fields it was asked to change. Bytes it doesn't know keep their value, and a test checks this. If nothing would change, it writes nothing.
- The official app also sets some other bytes in the same write, for its custom mode among others. Unkludge leaves those alone.

**Effects** on all 29 models: 1 to 17, by the names in `unkludge info`, which are Unkludge's own. Effect 18 is the custom mode, which Unkludge recognizes but doesn't offer. Which controls an effect has comes from the model data.

**Brightness** bytes on all 29 models: 0, 4, 8, 12, 16, 20. **Speed:** 0 to 5, default 1.

**Sleep** byte, with the labels the official app gives it:

| Byte | Label |
|---|---|
| `0x0A` | 5 min |
| `0x14` | 10 min |
| `0x28` | 20 min |
| `0x3C` | No sleep |

A byte that isn't one of these shows as raw, and is kept when other settings change.

## Palettes

```
09 0A 00 00 01 00 A4 01 <420 bytes>
```

20 entries of 21 bytes, each 7 colours as `R G B`. The entry's index is the effect number, and entry 0 is all zero. They're written all together, after the config if both change.

The official app has no command to read them back, so whoever writes them must keep a copy. A custom colour goes into slot 0 of its effect's entry, and the effect is set to use slot 0.

## Clock

```
09 0B 00 00 01 00 0E 00 <14 bytes>
```

| Byte | Meaning |
|---|---|
| 0 | System volume, in percent. Unkludge sends 0 |
| 1, 2 | CPU and memory load, in percent. Unkludge sends 0 |
| 3, 4 | Year, little-endian |
| 5 to 9 | Month, day, hour, minute, second |
| 10 | Day of the week, Sunday is 0 |
| 11 to 13 | Unkludge sends 0 |

The official app sends the time on connecting, with bytes 0 to 2 at 0. It doesn't send the clock during a screen upload. There's no ack, so a sent clock can only be checked by looking at the screen.

Example, for 2026-10-29 01:24:07, a Thursday:

```
09 0B 00 00 01 00 0E 00 00 00 00 EA 07 0A 1D 01 18 07 04
```

## Screen

Every size works the same way in the official app.

**Pixels:** RGB565, big-endian (`RRRRRGGG GGGBBBBB`), row by row from the top left. A frame is exactly `width * height * 2` bytes, with no header or padding.

| Screen | Models | Bytes per frame | Chunks per frame | Bytes in the last chunk |
|---|---|---|---|---|
| 320×172 | 16 | 110,080 | 215 | 512 |
| 240×135 | 9 | 64,800 | 127 | 288 |
| 240×240 | 3 | 115,200 | 225 | 512 |
| 128×128 | 1 | 32,768 | 64 | 512 |

**Chunk:**

```
09 0C 00 <frame> <idx> <check> <len lo> <len hi> <payload>
```

`frame` and `idx` are one byte each, counting from 0. `len` is the true length, and the rest of the 520 bytes is zero. `check` is 0, or on models with `crc = true` (the M87 family), `0xFF` minus the sum of the payload bytes, modulo 256. It covers the payload only.

**Control:**

```
09 0D 00 00 01 00 05 00 <(state << 6) | slot> <frames lo> <frames hi> <delay lo> <delay hi>
```

| State | Byte | Meaning |
|---|---|---|
| 1 | `0x40` | Start |
| 0 | `0x00` | Finish |
| 2 | `0x80` | Abort |

The slot is always 0.

**Delay:** the finish carries the delay between frames in milliseconds. The start carries that less the model's `draw_frame_time`, and at least 0. So the two differ on the S85, where that value is 180.

**Thinning out:** with more than 6 frames and a delay shorter than `draw_frame_time`, let `t = delay * frames`. If `t >= draw_frame_time`, only `t / draw_frame_time` frames get sent, evenly spaced and rounded to the nearest, keeping the first and last, unless only one remains, which is the first. The frame numbers on the wire then count those. Otherwise the frames stay as they are.

**Ack:**

```
09 0A 06 <status> <idx> <check> ...
```

- A chunk is accepted when `status` isn't 0, `idx` matches and, with `crc = true`, `check` matches.
- A `0x0D` is accepted when `status` isn't 0.
- Other input reports aren't acks.

**An upload, step by step:**

1. Wait 100 ms.
2. Send the start and wait for its ack. With no ack, send it again up to 3 more times, 200 ms apart. If the keyboard refuses it, stop there. In either failure no abort follows, and the 500 ms of the last step still applies.
3. Wait 120 ms.
4. Send each frame's chunks, each one waiting for its ack. Cancelling is checked before each frame and after each chunk.
5. Wait 50 ms.
6. Send the finish, or the abort if a chunk failed or the upload was cancelled, and wait for its ack. The official app ignores how this one ends. Unkludge warns if the finish isn't confirmed.
7. Wait 500 ms.

Each ack wait lasts up to 8 s. An ack that doesn't accept a chunk sends the chunk again: the official app has no limit on that, and Unkludge stops after 5. No ack for a chunk in that time fails the upload and sends the abort.

**`kind = 2` models**, the N99 family (`01B8`, `025E`, `025F`): `0x0D` goes to the main collection and waits for its ack, then goes to the second collection and waits for its ack. `0x0C` goes only to the second collection. The clock and the config stay on the main one. To find the second collection, each candidate must take 520-byte feature reports and answer `09 82 01 00 01 00 06 00` with the same 6 bytes as the main collection.

## Never sent

Unkludge has no command for any of these, and no way to send arbitrary bytes:

- Firmware updates, or entering a bootloader. This will never be added.
- A factory reset.
- Key maps, macros, per-key colours or live effects, in this version.
- Any command byte not in the [table above](#commands-and-their-status), or any report ID other than `0x09`. A test runs every flow and fails if anything else goes out.

It also never opens any collection but the ones under [Transport](#transport). On Windows each collection has its own path, so only those open. On Linux a hidraw node is the whole interface, so the list of commands is what keeps the others from being touched.

## Not confirmed

These are guesses or open questions. A capture or a test on hardware can settle them.

- Whether acks can arrive without their leading report ID on some platform. Unkludge accepts both forms.
- Whether the other 28 models have usage page `0xFF02`, usage 2, as the S85 ES does. The official app also accepts other usages, which Unkludge doesn't open until one is seen.
- What the firmware does with the sleep byte. It may count in 30-second units, which fits the first three labels. What `0x3C` does is unknown, and it might be 30 minutes rather than no sleep.
- That a colour in slot 0 of a palette entry is what the keyboard shows for that effect.
- Why uploads are slow. The official app adds no pause between chunks, so the time goes on waiting for acks. An Unkludge upload hasn't been timed yet: `--timing` is there to measure it.
- What `draw_frame_time` means to the firmware. It changes what's sent, as above, and that's all that's known.
- Whether the firmware rotates the image on any model. Nothing on the computer's side does.
- What the slot field in `0x0D` is for. It's always 0.
- What a keyboard does if an upload just stops, with no abort, as when it's unplugged.
- Anything on the `kind = 2` models and the `crc = true` models: their paths come only from the analysis, and none has been run on such a keyboard. Capturing `0x82` needs someone who owns an N99, as the S85 has no reason to send it.
