# VM test harness — simulate the Raspberry Pi

Plain QEMU + cloud-init (no Vagrant, no libvirt) to validate the production
installer ([Pnex/pnex-deploy](https://github.com/Pnex/pnex-deploy)) on a
throw-away machine before anything is pushed or flashed on real hardware.

| Arch | How | Speed | Use it for |
|---|---|---|---|
| `arm64` (default) | `qemu-system-aarch64 -M virt`, 4× Cortex-A72 (a Pi 4 core), UEFI, TCG emulation on x86 | slow: boot ~5-10 min, image pulls and first start much longer | faithful Pi check: arm64 images, memory profile |
| `amd64` | `qemu-system-x86_64` with KVM | boots in ~15 s | iterating on `install.sh` |

Guest: Debian 13 (trixie) generic cloud image, hostname `pnex-pi`, user
`pnex` (passwordless sudo), 4 vCPU / 4 GB RAM / 32 GB qcow2 overlay (the
downloaded base image is never modified).

## Host setup (Ubuntu)

```bash
sudo apt install qemu-system-x86 qemu-utils cloud-image-utils   # amd64 VM
sudo apt install qemu-system-arm qemu-efi-aarch64               # arm64 VM
sudo usermod -aG kvm "$USER"                                    # KVM without root (re-login)
```

Images and VM disks live in `deploy/vm/.cache/` (gitignored). Set
`PNEX_VM_CACHE=/some/dir` to keep them elsewhere. SSH uses a dedicated key
generated in the cache (plus your `~/.ssh/id_*.pub` if present).

## Usage

```bash
task vm:up ARCH=amd64                       # or: deploy/vm/up.sh --arch amd64
task vm:install                             # copy ../pnex-deploy into the VM, run its install.sh
task vm:ssh                                 # shell; `task vm:ssh -- sudo pnexctl status`
task vm:down                                # graceful power-off, disk kept
task vm:destroy                             # delete the VM disk (base image kept)
```

`install-in-vm.sh` (task `vm:install`, `SOURCE=local|remote`):

- `--source=local` (default): tars the sibling checkout `../pnex-deploy`
  (override with `--deploy-dir`) into the VM and runs
  `install.sh --source=local`. This is how a recipe change is validated
  **before** it is pushed.
- `--source=remote`: runs the real one-liner
  `curl -fsSL https://raw.githubusercontent.com/Pnex/pnex-deploy/<ref>/install.sh | sudo bash -s -- …`
  (`--ref` to test a branch or tag).
- `--load-images TAG`: images not published yet? `docker save`s the local
  `pnex-server:dev` / `pnex-builder:dev` (`task docker:build`) into the VM,
  tagged as the published names, and installs with `--no-pull --tag TAG`. The
  local images must match the VM architecture (amd64 builds for the amd64 VM).
- Installer flags go after `--`; without any, a throw-away admin is used
  (`admin@example.com` / `pnex-vm-admin`, `--non-interactive`).

Examples:

```bash
deploy/vm/install-in-vm.sh --arch amd64 -- --dry-run --non-interactive --admin-user a@example.com
deploy/vm/install-in-vm.sh --arch amd64 --load-images dirty-local
deploy/vm/install-in-vm.sh --source=remote --ref main -- --non-interactive --admin-user a@example.com
task vm:install ARCH=amd64 -- --storage=s3 --non-interactive --admin-user a@example.com
```

## Networking

Default: QEMU user networking (NAT) with forwards on the host loopback:

| Host | VM |
|---|---|
| `127.0.0.1:2222` | 22 (SSH) |
| `127.0.0.1:18443` | 443 (`--guest-https-port` to change) |
| `127.0.0.1:18080` | 80 |

Change them with `--ssh-port`, `--https-port`, `--http-port` (defaults avoid the dev stack,
which uses 8080/8443 for Rauthy and the edge). The installer's
health checks run **inside** the VM, so forwarding is only for you.

Browsing from the host: the OIDC login redirects to the install's origin
(`https://pnex-pi.local`), which the host cannot reach through a port
forward. Either install with a matching origin:

```bash
deploy/vm/up.sh --arch amd64 --https-port 8443 --guest-https-port 8443 --http-port 8088
deploy/vm/install-in-vm.sh -- --non-interactive --admin-user a@example.com \
  --domain localhost --https-port 8443
# → https://localhost:8443 on the host
```

or use a bridge (below).

### Bridged mode (real ESP devices on the LAN)

To let real devices reach the VM, attach it to a host bridge so it gets its
own LAN address (DHCP) and announces `pnex-pi.local`:

```bash
# once: a bridge enslaving the wired interface (NetworkManager)
sudo nmcli con add type bridge ifname br0 con-name br0
sudo nmcli con add type bridge-slave ifname enp0s31f6 master br0
sudo nmcli con up br0
# once: allow qemu-bridge-helper to use it
echo "allow br0" | sudo tee -a /etc/qemu/bridge.conf
sudo chmod u+s /usr/lib/qemu/qemu-bridge-helper

deploy/vm/up.sh --arch arm64 --net bridge:br0
```

Wi-Fi interfaces generally cannot be bridged; use a wired link. SSH then goes
to `pnex-pi.local:22` (mDNS).

## USB passthrough (flash a board from inside the VM)

```bash
lsusb                                   # e.g. 10c4:ea60 Silicon Labs CP210x
deploy/vm/up.sh --arch amd64 --usb 10c4:ea60
```

QEMU opens `/dev/bus/usb/BBB/DDD` itself, so your user needs write access to
that node, otherwise the device is silently not attached. A udev rule does it
(then unplug/replug):

```bash
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="10c4", ATTR{idProduct}=="ea60", MODE="0660", GROUP="plugdev"' \
  | sudo tee /etc/udev/rules.d/60-pnex-vm-usb.rules
sudo udevadm control --reload-rules
```

Browser flashing (Web Serial) happens on the client, not on the server: USB
passthrough is only useful to test server-side tooling or serial capture from
the VM.

## Files

| Script | Role |
|---|---|
| `up.sh` | download the image, create the overlay + cloud-init seed, boot, wait for SSH |
| `install-in-vm.sh` | run the pnex-deploy installer in the VM (local checkout or remote one-liner) |
| `ssh.sh` | interactive shell or one-off command |
| `down.sh` | ACPI power-off through QMP (kill after 120 s) |
| `destroy.sh` | down + delete disk/seed (`--all`: also base images and key) |
| `lib.sh` | shared helpers |
