# This Azure deployment

The watch service is deployed in **India South Central (Hyderabad)**:

- Subscription: the currently selected Azure for Students subscription.
- Resource group: `nvplay-watch-india`.
- VM: `nvplay-watch`, Ubuntu 24.04, `Standard_B4as_v2` (4 vCPUs, 16 GB RAM).
- OS disk: 32 GB Standard HDD.
- Public address: `https://nvplay-watch-97eed9f8.indiasouthcentral.cloudapp.azure.com`.
- SSH user: `nvplay`.
- Application directory on the VM: `/home/nvplay/app`.

The VM was increased for the WebRTC forwarding workload. This size is still
burstable (40% baseline across four vCPUs); non-burstable D-series allocation was
rejected in this subscription/region. Monitor CPU credits and total forwarding
load for sustained groups. The 15-minute idle deallocation timer remains enabled.

The same Azure hostname serves HTTPS and TURN on their separate ports. Coturn
uses Azure's public/private IP mapping and listens on the VM's private interface.
The Azure firewall permits HTTPS, TURN TCP/UDP 3478 and UDP relay ports
49160–49259 plus UDP 50000 for the forwarding server. SSH is restricted to the deploying computer's public IP.

## Local app configuration

The macOS settings folder is `~/Library/Application Support/nvplay`:

- `watch-server.txt`: the service address.
- `watch-host-key.txt`: the private host key read by nvplay on startup.
- `watch-deployment.json`: private deployment details, including the TURN secret.

The credential files have owner-only permissions. The app also accepts the
`NVPLAY_WATCH_URL` and `NVPLAY_WATCH_HOST_KEY` environment variables. Restart
nvplay after changing its settings files. Friends receive only session invites.

A dedicated SSH key is at `~/.ssh/nvplay_azure`; known host keys are kept in
`~/.ssh/nvplay_azure_known_hosts`.

## Check or manage the service

```sh
ssh -i ~/.ssh/nvplay_azure \
  -o UserKnownHostsFile=~/.ssh/nvplay_azure_known_hosts \
  nvplay@nvplay-watch-97eed9f8.indiasouthcentral.cloudapp.azure.com

cd /home/nvplay/app/deploy/watch
sudo docker compose ps
sudo docker compose logs --tail=100 signaling caddy
```

To stop the VM from your Mac:

```sh
az vm deallocate --resource-group nvplay-watch-india --name nvplay-watch
```

This takes the watch service offline and stops VM compute usage. Disk and public
IP resources remain allocated and can still incur charges. To start it again:

```sh
az vm start --resource-group nvplay-watch-india --name nvplay-watch
```

The static public IP remains assigned and Compose restarts the service containers.
If your home/public IP changes, update the `ssh-from-owner` rule in the VM's
`nvplay-watchNSG` network security group before SSH will work again.


## Automatic idle shutdown and wake-up

The VM now runs `nvplay-watch-idle.timer` once a minute. After **15 minutes with
no host sessions**, the signaling service grants a short shutdown lease and the
VM requests Azure **deallocation** using its system-assigned managed identity.
An open party counts as active even if no viewer is connected. Ending the last
party starts a fresh idle interval. Health checks do not keep it awake.

The custom role `nvplay-watch-self-deallocate` permits only
`Microsoft.Compute/virtualMachines/deallocate/action`, assigned on this VM.
It cannot start, delete, or manage other resources. The idle script lives at
`/opt/nvplay-watch/idle.py`; `/etc/nvplay-watch-idle.json` is root-only and holds
the VM resource ID and existing host key. The implementation and systemd units
are in [azure/](azure/). Never commit the private configuration.

The host app checks service health before creating a party. If this configured
Azure deployment is asleep, it starts the VM using the owner's already signed-in
Azure CLI, then waits for HTTPS readiness. The sidebar shows wake-up progress;
End session cancels the wait. Cold starts can take a couple of minutes.
`watch-deployment.json` must contain `url`, `resource_group`, `vm`, and the
`subscription` UUID. A custom service URL never wakes this deployment.
Guests do not need Azure credentials; only the host starts a sleeping service.
If the owner's CLI session expires, run `az login` and retry.

The idle endpoints are bearer-authenticated, exposed to the VM only through
`127.0.0.1:8787`, and blocked at Caddy. The shutdown lease and session creation
share a lock to prevent a new room racing a shutdown. A failed Azure request's
lease expires after 90 seconds so the service can accept sessions again.

```sh
sudo systemctl status nvplay-watch-idle.timer
sudo journalctl -u nvplay-watch-idle.service --no-pager -n 20
sudo python3 /opt/nvplay-watch/idle.py --check
```

To change the idle grace, set `WATCH_IDLE_SECONDS` in the private Compose `.env`
(default 900; minimum 60), then run `sudo docker compose up -d signaling`.
To disable automatic sleeping, run `sudo systemctl disable --now nvplay-watch-idle.timer`.
Deallocation stops compute billing, while the retained disk and public IP can
still incur charges: [Azure power states and billing](https://learn.microsoft.com/en-us/azure/virtual-machines/states-billing).
