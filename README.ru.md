# remna-sub-injector

**Remnawave Subscription Injector Proxy** — добавляет дополнительные протоколы (Hysteria2, TrustTunnel, SOCKS, MTProto и др.) в единую подписку [Remnawave](https://docs.rw) без изменения upstream-сервера.

## Что делает

Когда VPN-клиент запрашивает список подписки, инжектор:

1. Пересылает запрос на upstream-сервер подписок без изменений
2. Проверяет заголовки запроса (например, `User-Agent`) по настроенным правилам
3. Если правило совпало и ответ является base64-закодированным списком подписки — декодирует его, добавляет настроенные дополнительные ссылки и перекодирует перед отправкой клиенту
4. Если ни одно правило не совпало или ответ является YAML/JSON конфигом — пропускает без изменений

Это позволяет добавлять дополнительные ссылки (например, собственные Hysteria2 или VLESS узлы) в списки подписок без модификации upstream-сервера, при этом избирательно — для каждого клиентского приложения свои ссылки.

## Как работает инъекция

- Тело ответа должно быть base64-закодированным списком прокси URI, по одному на строку
- Инжектор декодирует тело из base64, добавляет дополнительные ссылки в конец и перекодирует обратно
- YAML и JSON никогда не модифицируются (конфиги Clash/Sing-Box)
- Правила проверяются по порядку; побеждает первое совпавшее
- Если источник ссылок недоступен или пуст, ответ передаётся без изменений

## Заметки по безопасности

- **Закройте порт от внешней сети.** Инжектор не имеет встроенной аутентификации — безопасность строится исключительно на секретности токена подписки в URL. Убедитесь, что порт 3020 недоступен снаружи (правило firewall или изолированная Docker-сеть).
- **TLS на самом инжекторе отсутствует.** Трафик между клиентом и инжектором передаётся по plain HTTP, поэтому токены и ссылки идут в открытом виде. Если клиенты подключаются через интернет, поставьте перед инжектором reverse proxy (nginx, Caddy и т.д.) с TLS-сертификатом.

## Установка

### Вариант 1 — Docker Compose (рекомендуется)

**Шаг 1.** Склонируйте репозиторий:

```bash
git clone https://github.com/itwormz/remna-sub-injector /opt/remna-sub-injector
cd /opt/remna-sub-injector
```

**Шаг 2.** Скачайте бинарник в папку `bin/`:

```bash
mkdir -p bin
ARCH=$(uname -m)
case $ARCH in
  x86_64)  BINARY="sub-injector-linux-x86_64" ;;
  aarch64) BINARY="sub-injector-linux-aarch64" ;;
  *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac
curl -L https://github.com/itwormz/remna-sub-injector/releases/latest/download/${BINARY} \
  -o bin/sub-injector
chmod +x bin/sub-injector
```

**Шаг 3.** Создайте конфиг:

```bash
cp config.toml.example config.toml
```

Отредактируйте `config.toml` под свои настройки перед запуском.

**Шаг 4.** Подготовьте источник дополнительных ссылок.

В каждом правиле `config.toml` есть поле `links_source` — инжектор читает из него прокси URI и добавляет их в каждый подходящий ответ подписки. Доступно два варианта:

- **Локальный файл** — создайте файл и запишите по одному прокси URI на строку:
  ```bash
  mkdir -p data
  nano data/hysteria2-links.txt
  ```
- **Внешний URL** — укажите в `links_source` адрес `https://`, который возвращает ссылки в том же формате (по одной на строку).

Подробности — в разделе [Формат источника ссылок](#формат-источника-ссылок).

**Шаг 5.** Создайте `docker-compose.yml`:

```bash
cp docker-compose.yml.example docker-compose.yml
```

**Шаг 6.** Запустите:

```bash
docker compose up -d
```

### Вариант 2 — Бинарный файл + systemd

Готовые бинарники публикуются в [GitHub Releases](../../releases):

| Файл | Архитектура |
|---|---|
| `sub-injector-linux-x86_64` | x86_64 (большинство серверов) |
| `sub-injector-linux-aarch64` | ARM64 (Raspberry Pi, AWS Graviton и др.) |

**Шаг 1.** Скачайте бинарник:

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

Чтобы установить конкретную версию, замените `latest/download` на `download/v0.1.0` в URL.

**Шаг 2.** Создайте конфиг:

```bash
mkdir -p /opt/remna-sub-injector
curl -L https://github.com/itwormz/remna-sub-injector/releases/latest/download/config.toml.example \
  -o /opt/remna-sub-injector/config.toml
```

Отредактируйте `/opt/remna-sub-injector/config.toml` под свои настройки перед запуском.

**Шаг 3.** Создайте файл сервиса:

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

**Шаг 4.** Включите и запустите:

```bash
systemctl daemon-reload
systemctl enable --now sub-injector
systemctl status sub-injector
```

Просмотр логов:

```bash
journalctl -u sub-injector -f
```

## Конфигурация

Инжектор читает конфиг из TOML-файла. По умолчанию ищет `config.toml` в рабочей директории. Путь можно переопределить через переменную окружения `CONFIG_FILE`.

### Справочник по параметрам

| Ключ | Тип | Обязателен | По умолчанию | Описание |
|---|---|---|---|---|
| `upstream_url` | string | да | — | Базовый URL upstream-сервера подписок |
| `bind_addr` | string | нет | `0.0.0.0:3020` | Адрес и порт для прослушивания |
| `injections` | массив | да | — | Список правил инъекции (см. ниже) |

Каждое правило `[[injections]]`:

| Ключ | Тип | Описание |
|---|---|---|
| `header` | string | Название заголовка запроса для проверки (без учёта регистра) |
| `contains` | массив строк | Список подстрок — правило срабатывает, если значение заголовка содержит **любую** из них (без учёта регистра) |
| `links_source` | string | Путь к локальному файлу **или** `http(s)://` URL для получения дополнительных ссылок |

### Пример конфига

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

# Также поддерживается удалённый URL в качестве источника:
# [[injections]]
# header = "User-Agent"
# contains = ["hiddify"]
# links_source = "https://example.com/my-extra-links.txt"
```

## Формат источника ссылок

Каждый источник ссылок (файл или URL) должен содержать по одному прокси URI на строке:

```
hysteria2://password@1.2.3.4:443?obfs=salamander&obfs-password=secret#Мой-Узел-1
vless://uuid@5.6.7.8:443?security=tls#Мой-Узел-2
ss://base64encodedinfo@9.10.11.12:8388#Мой-Узел-3
```

Пустые строки и пробелы в начале/конце обрезаются автоматически.

## Сборка из исходников

Нативный бинарник (x86_64):

```bash
cargo build --release
```

ARM64 musl (для Alpine / серверов на aarch64):

Установите инструмент кросс-компиляции один раз:

```bash
cargo install cross
```

Затем выполните сборку:

```bash
cross build --release --target aarch64-unknown-linux-musl
```

Результат: `target/aarch64-unknown-linux-musl/release/sub-injector`
