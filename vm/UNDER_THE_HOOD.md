# Under the Hood

## Core Idea

`vm` runs QEMU as a normal user process:
- Hardware acceleration: KVM on Linux, HVF on macOS
- Networking: SLIRP user-mode NAT with SSH port forwarding (`host:<port>` to `guest:22`)

This avoids tap/bridge setup and keeps VM startup sudo-free in normal flows.

## Minimal QEMU Shape

```text
-machine <machine>,accel=<kvm|hvf>
-cpu host -m <memory> -smp <cpus>
-drive file=disk.qcow2,format=qcow2,if=virtio
-drive file=seed.iso,format=raw,if=virtio,media=cdrom
-netdev user,id=net0,hostfwd=tcp::<host-port>-:22
-device virtio-net-pci,netdev=net0
-pidfile qemu.pid -daemonize
```

## Cloud-Init

Ubuntu cloud images boot from a NOCLOUD seed ISO (volume label `cidata`) containing:
- `meta-data`
- `user-data`

`vm` generates these files per instance to set hostname, user, SSH key, and first-boot config.

## Disk and Port Allocation

- VM disks are qcow2 overlays backed by a cached base image (`qemu-img create -b ...`), so each new VM is small and fast to create.
- SSH ports are reserved with file locks under `~/.local/share/vm/instances/.ssh-port-locks/`.
- If a requested port is in use, `vm` increments to the next available port.
