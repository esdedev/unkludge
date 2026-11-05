# Contributing to Unkludge

## Where the data and the protocol come from

Plainly, so nobody has to guess:

- **The protocol** was worked out by static analysis of the official RK app, for interoperability with keyboards people own. [docs/PROTOCOL.md](docs/PROTOCOL.md) describes the behaviour in our own words. The notes from that analysis stay outside this repository, and no decompiled code is in it. The app wasn't run for this, and no captures were involved: capturing what it sends is still to do, as described [below](#capturing-usb-traffic).
- **The model data** in `data/models/` is generated from the config files of the official app, by `tools/kbini2toml`. It holds facts about each model, in our own format: USB IDs, screen size, frame limits, which controls each effect has, key positions. Nothing else from that app is included: no images, animations, interface text, programs or firmware. The names of the effects are ours.

Unkludge is unofficial and not affiliated with Royal Kludge.

## Testing a model

Every model in `unkludge models` starts as **experimental**, and most have never been tried with Unkludge. Owning one is the most useful thing you can bring.

Close the RK app first (`DeviceDriver.exe`). It polls the keyboard and sends its own settings back to it, and `unkludge` warns if it finds it running. Quit Unkludge's own app too, if it's running: it keeps a lock on the keyboard while connected, and `unkludge` refuses until it's gone.

To see what a command would send, without opening anything, add `--dry-run --pid <PID>`, such as `unkludge --dry-run --pid 0265 clock`. A dry run has no keyboard to read settings from, so it reads as if the lights were off: changing a brightness, speed or colour there also needs `--mode`.

On Linux, not being able to open the keyboard most likely means permissions. `packaging/linux/70-unkludge.rules`, in this repository, grants them: the window's **Grant access** button installs it, and the [README](README.md#linux-access) says how to do it by hand.

Test in this order, from least to most risky, and stop at the first thing that looks wrong:

1. **`unkludge info`**. Only reads. Shows the model and its settings. Check they match what the keyboard is doing.
2. **`unkludge light`**, with no options. Only reads.
3. **`unkludge report`**. Only reads. Save the output: its config block lets you put your settings back by hand if a later step changes them.
4. **`unkludge clock`**. Reads the settings, then sends the time, and the keyboard doesn't answer. Look at its screen to see if it changed.
5. **`unkludge light --mode 2`**, then a brightness, a speed, a colour and a sleep time. Each writes settings that stay on the keyboard. `--rgb` also gives the other effects Unkludge's default colours: any colours set in the RK app are replaced, as they can't be read first.
6. **`unkludge screen <file>`**. Replaces what's on the screen. Start with one small image, then a short GIF. It can take minutes: add `--timing` to measure it.

### Is it safe?

As far as is known, but the honest answer is that nobody has tried most of this yet.

- **Unkludge never updates firmware**, and never will. It has no command for that, nor for a factory reset, nor any way to send arbitrary bytes.
- It only sends the commands in [docs/PROTOCOL.md](docs/PROTOCOL.md). No USB capture has confirmed them yet, and the table there says what has really been run, on which keyboard.
- Settings are read first and written back with only what you asked to change. If the read fails or looks wrong, nothing is written: not settings, not the clock, not a screen upload.
- Ctrl+C during `unkludge screen` tells the keyboard to abort, which can take a few seconds if it has stalled. Closing the terminal does the same, as far as the system leaves time. A second Ctrl+C quits at once, which, like unplugging the keyboard, leaves the upload unfinished. What a keyboard does then isn't known. Try unplugging it and plugging it back in, or uploading again, and say what happened.
- If a keyboard stops responding, unplug it and plug it back in, and say so in your report.

The `kind = 2` models (`01B8`, `025E`, `025F`) and the models with `crc = true` (the M87 family) upload differently from the S85 and have never been run on real hardware.

### Reporting what you found

Open an issue with the output of `unkludge report`, the commands you ran and what the keyboard did, good or bad.

The report contains the Unkludge version, the system, whether the RK app is running, the USB IDs and HID collections of any Royal Kludge device the system lists, the model, and the config block in hex: lighting settings and other keyboard settings. No names, accounts or files. Listing the collections sends nothing to the keyboard.

### How a model becomes `tested`

1. Someone who owns it runs the steps above, up to and including `screen`, and reports what happened in an issue.
2. A pull request changes `status = "experimental"` to `status = "tested"` in `data/models/<PID>.toml` and links that issue.

Only what was really run counts. If a step wasn't tried or didn't work, the model stays experimental, and the issue says why.

## Capturing USB traffic

Every command Unkludge sends must also appear in a capture of the RK app talking to a real keyboard before v0.1 is released. Captures of other models help too. In particular, `0x82` is only used by the N99 family, so capturing it needs someone who owns an N99. This is on Windows, where the RK app runs.

1. Install [Wireshark](https://www.wireshark.org/), and tick **USBPcap** when its installer offers it. Otherwise install it from [desowin.org/usbpcap](https://desowin.org/usbpcap/).
2. **Reboot.** USBPcap's driver only starts working after that.
3. Open Wireshark and start a capture on the USBPcap interface for the root hub your keyboard is on. If you don't know which, try each: the right one fills up when you press keys.
4. **Don't type anything private during a capture**, and treat the file as private. A capture records the keyboard's key presses too.
5. Open the RK app and wait for it to settle.
6. Make **one change at a time**, with **10 seconds between changes**, and write down the time and what you did. The gaps tell the changes apart from what the app sends on its own. Good ones to do: an effect, a brightness, a speed, a colour, a sleep time, replugging the keyboard with the app open and then changing the system volume, a small GIF upload, and cancelling one.
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

A model already marked `tested` stays that way.

[docs/MODELS.md](docs/MODELS.md) is generated from that data, and a test fails if it's out of date. After changing a model, regenerate it:

```
cargo run -p unkludge-cli -- models --markdown > docs/MODELS.md
```

In Windows PowerShell 5, `>` writes UTF-16, which that test reports as out of date. There, go through `cmd`, which works in any Windows shell:

```
cmd /c "cargo run -p unkludge-cli -- models --markdown > docs\MODELS.md"
```

The udev rule in `packaging/linux/70-unkludge.rules` lists the supported models by hand, and a test fails if it disagrees with the model data.
