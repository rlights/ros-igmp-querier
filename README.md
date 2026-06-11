# ros-igmp-querier

A VLAN-aware IGMP Querier designed to run as a container natively on MikroTik RouterOS.

## Problem

The built-in IGMP snooping querier in MikroTik RouterOS (`multicast-querier=yes` on a bridge) only transmits untagged IGMP general queries. In deployments utilizing tagged VLANs for multicast traffic, devices on those VLANs do not receive the untagged queries. This causes the switch multicast database (MDB) to expire, which halts multicast forwarding across the VLAN.

## Solution

This application is a minimal Rust binary that bypasses standard Linux networking by using a raw socket (`AF_PACKET`). When attached to a RouterOS bridge via a trunked veth interface, it manually crafts and injects 802.1Q tagged IGMPv2 General Query packets for each configured VLAN.

To prevent IP conflicts and handle IGMP querier elections reliably, it automatically derives a link-local source IP address (`169.254.X.Y`) where `X` and `Y` are the high and low bytes of the target VLAN ID.

## Environment Variables

The container is configured entirely via environment variables defined in RouterOS:

* `VLANS` (Required): A comma-separated list of the target VLAN IDs (e.g., `10,20,30`).
* `QUERIER_IP` (Optional): The source IP address injected into the IGMP query packets. Defaults to `dynamic` (derives `169.254.X.Y` based on the VLAN ID). Can be set to a static IPv4 address.
* `INTERVAL` (Optional): The time in seconds to wait between sending queries. Defaults to `125` (standard IGMP general query interval).
* `INTERFACE` (Optional): The internal container network interface to bind to. Defaults to `eth0`. If not found, the application automatically scans and binds to the first active, non-loopback interface.

## Building the Container

RouterOS 7.21+ requires a strict OCI image format. The provided Dockerfile utilizes a two-stage build to cross-compile the binary for ARM64 without relying on QEMU emulation.

To build the container and export it as a RouterOS compatible tarball, run the following command:

```bash
docker buildx build --platform linux/arm64 --provenance=false --output=type=oci,dest=ros-igmp-querier-arm64.tar -t ros-igmp-querier:arm64 .
```

The `--provenance=false` flag is mandatory. Without it, Docker buildx generates an OCI index containing build metadata which causes a `no config found in manifest` error during RouterOS extraction.

## RouterOS Configuration

Upload `ros-igmp-querier-arm64.tar` to your router storage.

1. Configure the environment variables to define your target VLANs.
```routeros
/container/envs add name="querier_env" key="VLANS" value="10,20,30"
```

2. Create the veth interface. A dummy IP is required by RouterOS.
```routeros
/interface veth add name="veth-querier" address="192.168.99.2/24" gateway="192.168.99.1"
```

3. Add the veth interface to your bridge and configure it as a tagged member in your VLAN table.
```routeros
/interface bridge port add bridge=bridge interface=veth-querier
/interface bridge vlan add bridge=bridge tagged=bridge,veth-querier vlan-ids=10,20,30
```

4. Instantiate the container. The `shm-size` parameter must be explicitly defined.
```routeros
/container add file="ros-igmp-querier-arm64.tar" interface=veth-querier envlist="querier_env" logging=yes root-dir=disk1/igmp-querier-root shm-size=8MiB
```

5. Start the container.
```routeros
/container start [find interface="veth-querier"]
```

Check the logs using `/log print where topics~"container"` to verify the application has detected the interface and is transmitting queries.