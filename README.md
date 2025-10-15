# elgato-streamdeck-async

Rust library for interacting with Elgato Stream Deck hardware.
Forked from [rust-elgato-streamdeck](https://github.com/OpenActionAPI/rust-elgato-streamdeck) with the objective of providing an async version by using [async-hid](https://github.com/HRKings/async-hid).

## udev rules for Linux

If you're using systemd on your system, you might have to install udev rules to allow connecting to devices from userspace.

You can do that by using the following command to copy this repo's included `40-streamdeck.rules` file into `udev/rules.d/`:

```shell
cp 40-streamdeck.rules /etc/udev/rules.d/
```

And then reloading udev rules:

```shell
sudo udevadm control --reload-rules
```

Unplugging and plugging back in the device should also help.

You also need to restart the user session to let user group changes to kick in.

## Example

Please look at the `examples/simple.rs` file.

## Supported Devices

As it stands, this library should support the following devices:

- Stream Deck Original
- Stream Deck Original V2
- Stream Deck XL
- Stream Deck XL V2
- Stream Deck Mini
- Stream Deck Mini Mk2
- Stream Deck Mk2
- Stream Deck Pedal
- Stream Deck Plus
- Stream Deck Neo
