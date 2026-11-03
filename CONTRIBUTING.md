# Contributing to Unkludge

## Where the data and the protocol come from

Plainly, so nobody has to guess:

- **The protocol** was worked out by static analysis of the official RK app, for interoperability with keyboards people own. [docs/PROTOCOL.md](docs/PROTOCOL.md) describes the behaviour in our own words. The notes from that analysis stay outside this repository, and no decompiled code is in it. The app wasn't run for this, and no captures were involved: capturing what it sends is still to do, as described [below](#capturing-usb-traffic).
- **The model data** in `data/models/` is generated from the config files of the official app, by `tools/kbini2toml`. It holds facts about each model, in our own format: USB IDs, screen size, frame limits, which controls each effect has, key positions. Nothing else from that app is included: no images, animations, interface text, programs or firmware. The names of the effects are ours.

Unkludge is unofficial and not affiliated with Royal Kludge.

## Testing a model

Every model in `unkludge models` starts as **experimental**, and most have never been tried with Unkludge. Owning one is the most useful thing you can bring.

Close the RK app first (`DeviceDriver.exe`). It polls the keyboard and sends its own settings back to it, and `unkludge` warns if it finds it running.

To see what a command would send, without opening anything, add `--dry-run --pid <PID>`, such as `unkludge --dry-run --pid 0265 clock`.

Test in this order, from least to most risky, and stop at the first thing that looks wrong:

1. **`unkludge info`**. Only reads. Shows the model and its settings. Check they match what the keyboard is doing.
2. **`unkludge light`**, with no options. Only reads.
3. **`unkludge report`**. Only reads. Save the output: its config block lets you put your settings back by hand if a later step changes them.
4. **`unkludge clock`**. Sends the time, and the keyboard doesn't answer. Look at its screen to see if it changed.
5. **`unkludge light --mode 2`**, then a brightness, a speed, a colour and a sleep time. Each writes settings that stay on the keyboard. `--rgb` also resets the colours of the other effects to the defaults.
6. **`unkludge screen <file>`**. Replaces what's on the screen. Start with one small image, then a short GIF. It can take minutes: add `--timing` to measure it.

### Is it safe?

As far as is known, but the honest answer is that nobody has tried most of this yet.

- **Unkludge never updates firmware**, and never will. It has no command for that, nor for a factory reset, nor any way to send arbitrary bytes.
- It only sends the commands in [docs/PROTOCOL.md](docs/PROTOCOL.md). No USB capture has confirmed them yet, and the table there says what has really been run, on which keyboard.
- Settings are read first and written back with only what you asked to change. If the read fails or looks wrong, nothing is written.
- `unkludge screen` has no cancel: stopping it with Ctrl+C, or unplugging the keyboard, leaves the upload unfinished. What a keyboard does then isn't known. Try unplugging it and plugging it back in, or uploading again, and say what happened.
- If a keyboard stops responding, unplug it and plug it back in, and say so in your report.

The `kind = 2` models (`01B8`, `025E`, `025F`) and the models with `crc = true` (the M87 family) upload differently from the S85 and have never been run on real hardware.

### Reporting what you found

Open an issue with the output of `unkludge report`, the commands you ran and what the keyboard did, good or bad.

The report contains the Unkludge version, the system, the model, its USB product ID and the config block in hex: lighting settings and other keyboard settings. No names, accounts or files.

### How a model becomes `tested`

1. Someone who owns it runs the steps above, up to and including `screen`, and reports what happened in an issue.
2. A pull request changes `status = "experimental"` to `status = "tested"` in `data/models/<PID>.toml` and links that issue.

Only what was really run counts. If a step wasn't tried or didn't work, the model stays experimental, and the issue says why.

## Capturing USB traffic

Every command Unkludge sends must also appear in a capture of the RK app talking to a real keyboard before v0.1 is released. Captures of other models help too. This is on Windows, where the RK app runs.

1. Install [Wireshark](https://www.wireshark.org/), and tick **USBPcap** when its installer offers it. Otherwise install it from [desowin.org/usbpcap](https://desowin.org/usbpcap/).
2. **Reboot.** USBPcap's driver only starts working after that.
3. Open Wireshark and start a capture on the USBPcap interface for the root hub your keyboard is on. If you don't know which, try each: the right one fills up when you press keys.
4. **Don't type anything private during a capture**, and treat the file as private. A capture records the keyboard's key presses too.
5. Open the RK app and wait for it to settle.
6. Make **one change at a time**, with **10 seconds between changes**, and write down the time and what you did. The gaps tell the changes apart from what the app sends on its own. Good ones to do: an effect, a brightness, a speed, a colour, a sleep time, syncing the clock, a small GIF upload, and cancelling one.
7. Stop the capture and save it as `.pcapng`.

To read it, the display filter `usb.idVendor == 0x258a` shows the keyboard's device address, and `usb.device_address == <address>` then shows only its traffic. Commands are control transfers (`SET_REPORT`, `GET_REPORT`), and acks arrive as interrupt transfers. Compare the bytes with what `unkludge --dry-run --verbose` prints for the same action.

**`*.pcapng` files never get committed**, and `.gitignore` keeps them out. Share only the reports concerned, as hex, with the time and the change you made. When a command is confirmed, update its column in [docs/PROTOCOL.md](docs/PROTOCOL.md).

## Ground rules

- **Behaviour only.** Code comments and docs describe what the keyboard does: bytes, offsets, timings. Never include decompiled code, or class or function names from vendor software.
- **Nothing copied but facts.** No vendor images, animations, interface text, programs or firmware.
- **Never** add firmware, bootloader, factory reset or raw-send commands.
- **No new commands without evidence.** A command goes in only once it's understood and seen in a capture, or is listed as pending in [docs/PROTOCOL.md](docs/PROTOCOL.md) until it is. Keep the facts apart from the guesses there.
- **Code comments in English.** Tests come with changes in behaviour.

## Building and checking

Run these before opening a pull request. They need to pass on Windows and Linux:

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The tests never touch a real keyboard: they use a simulated one or `--dry-run`.

To regenerate the model data, with the RK app installed:

```
cargo run -p kbini2toml -- "C:\Program Files (x86)\RK Keyboard Software" data/models
```

It writes every model as `experimental`, so check the diff and keep the `tested` lines.
