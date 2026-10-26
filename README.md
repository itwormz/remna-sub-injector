# remna-sub-injector

**🇬🇧 English** · [🇷🇺 Русский](README.ru.md)

[![Release](https://img.shields.io/github/v/release/itwormz/remna-sub-injector?style=for-the-badge&logo=github&label=Release&color=2563eb)](https://github.com/itwormz/remna-sub-injector/releases/latest)
[![GHCR](https://img.shields.io/badge/GHCR-AMD64%20%7C%20ARM64-0f766e?style=for-the-badge&logo=docker&logoColor=white)](https://github.com/itwormz/remna-sub-injector/pkgs/container/remna-sub-injector)
[![License: MIT](https://img.shields.io/badge/License-MIT-64748b?style=for-the-badge)](LICENSE)

**Remnawave Subscription Injector Proxy** — adds extra protocols (Hysteria2, TrustTunnel, SOCKS, MTProto, etc.) into a single [Remnawave](https://docs.rw) subscription without modifying the upstream server.

## What it does

When a VPN client fetches a subscription list, the injector:

1. Forwards the request to the upstream subscription server unchanged
2. Checks the request headers (e.g. `User-Agent`) against configured rules
3. If a rule matches and the response is a base64-encoded subscription list — decodes it, appends the configured extra links, and re-encodes it before sending back to the client
4. If no rule matches, or the response is a YAML/JSON config — passes it through untouched

This allows injecting additional links (e.g. your own Hysteria2 or VLESS nodes) into subscription lists without modifying the upstream server, and doing so selectively per client app.

## How injection works

- The response body is expected to be a base64-encoded newline-separated list of proxy URIs
- The injector base64-decodes the body, appends the extra links, and re-encodes
- YAML and JSON content types are never modified (Clash/Sing-Box config files)
- Injection rules are evaluated in order; the first matching rule wins
- If the links source is unreachable or empty, the response is passed through unchanged

## Security notes

- **Close the port from the public internet.** The injector has no built-in authentication — security relies entirely on the subscription token in the URL being secret. Make sure port 3020 is not reachable from the outside (firewall rule or a private Docker network).
- **No TLS on the injector itself.** Traffic between the client and the injector is plain HTTP, so tokens and links are transmitted in cleartext. Place a reverse proxy (nginx, Caddy, etc.) with a TLS certificate in front of the injector when clients connect over the internet.

## Installation

### Option 1 — Docker Compose (recommended)

**Step 1.** Create a working directory and download the configuration example:

```bash
mkdir -p /opt/remna-sub-injector
cd /opt/remna-sub-injector
curl -fsSL https://github.com/itwormz/remna-sub-injector/releases/download/v0.1.1/config.toml.example \
  -o config.toml.example
```

**Step 2.** Use the pre-built image [`ghcr.io/itwormz/remna-sub-injector:0.1.1`](https://github.com/itwormz/remna-sub-injector/pkgs/container/remna-sub-injector). Docker automatically selects `linux/amd64` or `linux/arm64`. No binary download or local build is required.

**Step 3.** Create the config:

```bash
cp config.toml.example config.toml
```

Edit `config.toml` before starting.

**Step 4.** Prepare your extra links.

Each injection rule in `config.toml` has a `links_source` field — the injector will read proxy URIs from it and append them to every matching subscription response. You have two options:

- **Local file** — create the file and put one proxy URI per line:
  ```bash
  mkdir -p data
  nano data/hysteria2-links.txt
  ```
- **Remote URL** — point `links_source` directly to an `https://` URL that returns the same one-URI-per-line format.

See [Links source format](#links-source-format) for details.

**Step 5.** Create `docker-compose.yml`:

Reference the pre-built GHCR image in `docker-compose.yml`:

```yaml
services:
  sub-injector:
    image: ghcr.io/itwormz/remna-sub-injector:0.1.1
    restart: unless-stopped
    ports:
      - "127.0.0.1:3020:3020"
    volumes:
      - ./config.toml:/app/config.toml:ro
      - ./data:/data:ro
```

The port is accessible only from the host, for example for a reverse proxy. If the reverse proxy runs in another container, connect both services to a shared Docker network and use `http://sub-injector:3020`; you can then omit `ports`. Set `upstream_url` to a subscription server address reachable from the container: `localhost` means the injector container itself.

**Step 6.** Start:

```bash
docker compose up -d
```

### Docker updates and rollback

Change the `image:` tag in `docker-compose.yml` to the desired version, then run:

```bash
docker compose pull
docker compose up -d
```

To roll back, select the previous published version and repeat these commands. Use a specific version tag instead of `latest` to control upgrades.

### Option 2 — Binary + systemd

Pre-built binaries are published in [GitHub Releases](https://github.com/itwormz/remna-sub-injector/releases):

| File | Architecture |
|---|---|
| `sub-injector-linux-x86_64` | x86_64 (most servers) |
| `sub-injector-linux-aarch64` | ARM64 (Raspberry Pi, AWS Graviton, etc.) |

**Step 1.** Download the binary:

```bash
ARCH=$(uname -m)
case $ARCH in
  x86_64)  BINARY="sub-injector-linux-x86_64" ;;
  aarch64) BINARY="sub-injector-linux-aarch64" ;;
  *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac
curl -L https://github.com/itwormz/remna-sub-injector/releases/latest/download/${BINARY} \
  -o /usr/local/bin/sub-injector
chmod +x /usr/local/bin/sub-injector
```

To install a specific version, replace `latest/download` with `download/v0.1.1` in the URL.

**Step 2.** Create the config:

```bash
mkdir -p /opt/remna-sub-injector
curl -L https://github.com/itwormz/remna-sub-injector/releases/latest/download/config.toml.example \
  -o /opt/remna-sub-injector/config.toml
```

Edit `/opt/remna-sub-injector/config.toml` before starting.

**Step 3.** Create the service file:

```bash
cat > /etc/systemd/system/sub-injector.service << 'EOF'
[Unit]
Description=remna-sub-injector
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/sub-injector
Environment=CONFIG_FILE=/opt/remna-sub-injector/config.toml
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
EOF
```

**Step 4.** Enable and start:

```bash
systemctl daemon-reload
systemctl enable --now sub-injector
systemctl status sub-injector
```

View logs:

```bash
journalctl -u sub-injector -f
```

## Configuration

### Configuration file and environment

Version `0.1.1` reads settings from a TOML file. The application's only environment variable is `CONFIG_FILE`: it selects the **file path**, not its contents.

| Deployment | Default path |
|---|---|
| GHCR container | `/app/config.toml` |
| Standalone binary | `config.toml` in the working directory |

To use another path in the container, match the environment variable to the mount:

```yaml
environment:
  CONFIG_FILE: /etc/sub-injector/config.toml
volumes:
  - ./config.toml:/etc/sub-injector/config.toml:ro
  - ./data:/data:ro
```

This is a fragment of the `sub-injector` service settings from the example above. The host file `./config.toml` must exist and be readable by the container's `app` user. A `links_source` of `/data/hysteria2-links.txt` refers to `./data/hysteria2-links.txt` on the host.

`upstream_url`, `bind_addr`, and `injections` are read from TOML. Individual environment overrides and full TOML content through env are not supported. `${NAME}` placeholders inside TOML are not expanded either. Docker Compose can use an `.env` file, but it does not replace the application's configuration file.

### Applying configuration changes

Configuration is read at startup. After editing TOML, restart the service:

```bash
docker compose restart sub-injector
```

After changing the `CONFIG_FILE` path or mounts, recreate the container:

```bash
docker compose up -d --force-recreate sub-injector
```

A missing or invalid TOML file terminates the application. Inspect errors with:

```bash
docker compose logs --tail=100 sub-injector
```

### Config reference

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `upstream_url` | string | yes | — | Base URL of the upstream subscription server |
| `bind_addr` | string | no | `0.0.0.0:3020` | Address and port to listen on |
| `injections` | array | yes | — | List of injection rules (see below) |

Each `[[injections]]` rule:

| Key | Type | Description |
|---|---|---|
| `header` | string | Request header name to match against (case-insensitive) |
| `contains` | array of strings | List of substrings — rule matches if the header value contains **any** of them (case-insensitive) |
| `links_source` | string | Local file path **or** `http(s)://` URL to fetch extra links from |

### Example config

```toml
upstream_url = "http://upstream:2096"
bind_addr = "0.0.0.0:3020"

[[injections]]
header = "User-Agent"
contains = ["hiddify", "happ", "nekobox", "nekoray", "sing-box", "v2rayng"]
links_source = "/data/hysteria2-links.txt"

[[injections]]
header = "User-Agent"
contains = ["clash.meta", "mihomo"]
links_source = "/data/clash-links.txt"

# Remote URL is also supported:
# [[injections]]
# header = "User-Agent"
# contains = ["hiddify"]
# links_source = "https://example.com/my-extra-links.txt"
```

## Links source format

Each links source (file or URL) must contain one proxy URI per line:

```
hysteria2://password@1.2.3.4:443?obfs=salamander&obfs-password=secret#My-Node-1
vless://uuid@5.6.7.8:443?security=tls#My-Node-2
ss://base64encodedinfo@9.10.11.12:8388#My-Node-3
```

Blank lines and leading/trailing whitespace are stripped automatically.

## Building from source

Native binary (x86_64):

```bash
cargo build --release --locked
```

ARM64 musl (for Alpine / aarch64 servers):

Install the cross-compilation tool once:

```bash
cargo install cross
```

Then build:

```bash
cross build --release --locked --target aarch64-unknown-linux-musl
```

Output: `target/aarch64-unknown-linux-musl/release/sub-injector`

## Building and publishing releases

CI builds one release binary for each of Linux AMD64 and ARM64. The same binaries are published for systemd installations and packaged into GHCR images; the Dockerfile only packages them, without compiling Rust. See the [maintainer guide](.github/RELEASING.md) for release steps and proposed repository protections.
