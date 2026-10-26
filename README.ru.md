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

**Шаг 2.** Используйте готовый образ [`ghcr.io/itwormz/remna-sub-injector:0.1.1`](https://github.com/itwormz/remna-sub-injector/pkgs/container/remna-sub-injector). Docker автоматически выберет `linux/amd64` или `linux/arm64`. Скачивание бинарника и локальная сборка не нужны.

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

Укажите готовый образ из GHCR в `docker-compose.yml`:

```yaml
services:
  sub-injector:
    image: ghcr.io/itwormz/remna-sub-injector:0.1.1
    restart: unless-stopped
    ports:
      - "3020:3020"
    volumes:
      - ./config.toml:/app/config.toml
      - ./data:/data
```

**Шаг 6.** Запустите:

```bash
docker compose up -d
```

### Обновление и откат Docker

Измените тег `image:` в `docker-compose.yml` на нужную версию, затем выполните:

```bash
docker compose pull
docker compose up -d
```

Для отката укажите предыдущую опубликованную версию и повторите команды. Используйте конкретный тег версии вместо `latest`, чтобы контролировать обновления.

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

### Переменные окружения в версии 0.1.1

| Переменная | Значение | По умолчанию |
|---|---|---|
| `CONFIG_FILE` | Путь к существующему TOML-файлу **внутри контейнера** | `config.toml` относительно рабочей директории; в образе это `/app/config.toml` |

`CONFIG_FILE` принимает путь, а не содержимое конфига. Переменные `UPSTREAM_URL`, `PORT`, `EXTRA_LINKS_FILE` и `CONFIG_TOML` приложение напрямую не читает. Параметры `upstream_url`, `bind_addr` и правила `injections` задаются в TOML.

### Вариант A — файл и CONFIG_FILE

Сохраните настройки в `./config.toml` на хосте. Пример Compose с произвольным путём внутри контейнера:

```yaml
services:
  sub-injector:
    image: ghcr.io/itwormz/remna-sub-injector:0.1.1
    restart: unless-stopped
    environment:
      CONFIG_FILE: /etc/sub-injector/config.toml
    ports:
      - "127.0.0.1:3020:3020"
    volumes:
      - ./config.toml:/etc/sub-injector/config.toml:ro
      - ./data:/data:ro
```

Здесь `./config.toml` — файл на хосте, `/etc/sub-injector/config.toml` — его путь в контейнере. `:ro` разрешает приложению только чтение. Файл должен существовать до запуска и быть доступен для чтения пользователю `app`, от которого работает образ. `links_source = "/data/hysteria2-links.txt"` означает файл `./data/hysteria2-links.txt` на хосте.

`127.0.0.1:3020:3020` публикует порт только на самом хосте, например для reverse proxy. Если reverse proxy — соседний контейнер, подключите оба сервиса к общей Docker-сети и обращайтесь к `http://sub-injector:3020`; публикацию `ports` тогда можно убрать. `localhost` в `upstream_url` относится к самому контейнеру инжектора: для соседнего сервиса укажите его имя в общей сети.

### Вариант B — весь TOML через env без монтирования конфига

В образе 0.1.1 это можно сделать через стартовую shell-команду. Она записывает многострочную переменную `CONFIG_TOML` во временный файл, затем запускает приложение с `CONFIG_FILE`. Это пример настройки Compose, а не встроенная поддержка `CONFIG_TOML` приложением.

```yaml
services:
  sub-injector:
    image: ghcr.io/itwormz/remna-sub-injector:0.1.1
    restart: unless-stopped
    environment:
      CONFIG_FILE: /tmp/sub-injector.toml
      CONFIG_TOML: |
        upstream_url = "https://subscriptions.example.com"
        bind_addr = "0.0.0.0:3020"

        [[injections]]
        header = "User-Agent"
        contains = ["hiddify", "happ"]
        links_source = "https://example.com/extra-links.txt"
    entrypoint: ["/bin/sh"]
    command:
      - -ec
      - |
        umask 077
        printf '%s\n' "$$CONFIG_TOML" > "$$CONFIG_FILE"
        exec /app/sub-injector
    ports:
      - "127.0.0.1:3020:3020"
```

Замените оба URL на свои. В примере дополнительные ссылки берутся по HTTPS, поэтому volume `/data` не нужен. Для локального `links_source` добавьте монтирование `./data:/data:ro` из варианта A. `/tmp` должен оставаться доступным для записи; при `read_only: true` добавьте `tmpfs: ["/tmp"]`.

`$$` сохраняет знак `$` при обработке Compose: переменные подставляет shell уже внутри контейнера. `exec` передаёт процессу приложения сигналы остановки. Содержимое env видно пользователям с доступом к инспектированию контейнера; для конфигурации с секретами используйте файл с ограниченным доступом.

### .env, подстановка значений и применение изменений

- `.env` используется Docker Compose для подстановки `${NAME}` в Compose-файл. Само наличие `.env` не передаёт все его значения приложению: используйте `environment` или `env_file`.
- Даже переданный через `env_file` параметр `UPSTREAM_URL` не изменит настройки программы: версия 0.1.1 читает их из TOML. Для варианта B нужно передать `CONFIG_TOML` и сохранить стартовую команду из примера.
- Подстановка `${NAME}` **внутри примонтированного TOML-файла не выполняется**. Например, `upstream_url = "${UPSTREAM_URL}"` не превращается автоматически в адрес из env. Подстановка Compose работает в YAML, в том числе в многострочном `CONFIG_TOML` варианта B. Для буквального `$` в YAML используйте `$$`.
- Основной TOML читается при старте. После изменения примонтированного файла выполните `docker compose restart sub-injector`. После изменения `environment`, `env_file` или Compose выполните `docker compose up -d --force-recreate sub-injector`.
- Отсутствующий или некорректный TOML приводит к завершению приложения; ошибку можно посмотреть через `docker compose logs --tail=100 sub-injector`.

Перед запуском проверьте синтаксис Compose: `docker compose config --quiet`. Эта команда проверяет Compose, но не схему TOML и не доступность upstream. Подробнее: [переменные Compose](https://docs.docker.com/compose/how-tos/environment-variables/variable-interpolation/) и [экранирование $$](https://docs.docker.com/reference/compose-file/interpolation/).

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
