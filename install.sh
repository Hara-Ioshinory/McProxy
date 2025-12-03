#!/bin/sh
set -eu

# Путь к бинарю и сервису
BIN_SRC="./mc-proxy"
BIN_DST="/usr/local/bin/mc-proxy"
SYMLINK="/usr/bin/mc-proxy"
SERVICE_FILE="/etc/systemd/system/mc-proxy.service"
CONFIG_DIR="/etc/mc-proxy"
CONFIG_FILE="$CONFIG_DIR/proxy.json"

# 1. Установить бинарь
if [ ! -f "$BIN_SRC" ]; then
    echo "Ошибка: $BIN_SRC не найден"
    exit 1
fi

install -m 755 "$BIN_SRC" "$BIN_DST"

# 2. Создать симлинк в /usr/bin для надёжности
ln -sf "$BIN_DST" "$SYMLINK"

# 3. Создать systemd unit
cat > "$SERVICE_FILE" <<'EOF'
[Unit]
Description=MC Proxy Service
After=network.target
Wants=network-online.target
    
[Service]
Type=simple
ExecStart=/usr/local/bin/mc-proxy
# Перезапускать только при ошибке запуска/выполнения, но не бесконечно
Restart=on-failure
RestartSec=5
# Ограничение количества попыток рестарта: после StartLimitBurst не будет новых попыток в интервале
StartLimitBurst=3
StartLimitIntervalSec=60
# Запуск от непривилегированного пользователя
User=nobody
Group=nogroup
# Безопасные опции (по желанию)
ProtectSystem=full
NoNewPrivileges=yes

[Install]
WantedBy=multi-user.target
EOF

# Перезагрузить конфигурацию systemd
systemctl daemon-reload

# Включить автозапуск (не обязательно стартовать)
systemctl enable mc-proxy.service

# 4. Создать папку и конфиг (если ещё нет)
mkdir -p "$CONFIG_DIR"
cat > "$CONFIG_FILE" << 'EOF'
{
    "tcp_port": 25526,
    "udp_port": 24454,
    "endpoints": {
        "185.24.55.33": {
            "fractal": [54636, 24454]
        }
    }
}
EOF

# 5. Попытаться запустить сервис и проверить состояние
echo "Запускаю mc-proxy.service..."
# Не прерываем выполнение скрипта при неудаче — будем проверять состояние вручную
if ! systemctl start mc-proxy.service; then
    echo "systemctl start вернул ошибку"
fi

# Небольшая пауза, чтобы сервис успел перейти в состояние
sleep 1

# Проверяем состояние: если активен — всё ок, иначе останавливаем чтобы он был в stopped/inactive
if systemctl is-active --quiet mc-proxy.service; then
    echo "mc-proxy успешно запущен и активен."
else
    echo "mc-proxy не запустился. Принудительно останавливаю сервис, чтобы он отображался как stopped."
    # Останавливаем (на случай, если systemd всё ещё пытается рестартовать или в failed)
    systemctl stop mc-proxy.service || true
    # Можно также показать статус для отладки
    echo "Текущий статус systemd:"
    systemctl --no-pager status mc-proxy.service || true
    echo "mc-proxy оставлен в состоянии stopped (inactive). Проверьте логи: journalctl -u mc-proxy.service"
fi

echo "Установка завершена."
