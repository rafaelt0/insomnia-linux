# Upgrade from the previous name

Earlier source builds of this port used the command and service name
`insomnia`. End that version's session and run `./uninstall.sh` from its older
checkout before installing Insomnia Awake. Keep the default option that
preserves your settings.

To reuse your settings, run these commands before the new installer:

```bash
config_root="${XDG_CONFIG_HOME:-$HOME/.config}"
mkdir -p "$config_root/insomnia-awake"
chmod 700 "$config_root/insomnia-awake"
install -m 600 "$config_root/insomnia/config.json" "$config_root/insomnia-awake/config.json"
```

The new installer and uninstaller manage only the `insomnia-awake` files.
They do not migrate or remove another application's `insomnia` command.
