# Guía de despliegue y operación

Documentación técnica para quien despliega y mantiene su propia instancia de Roberto's Watching. Para una introducción al proyecto, consulte el [README](README.md).

## Comandos de administración

Disponibles solo para el chat definido en `ADMIN_CHAT_ID`.

| Comando | Descripción |
| --- | --- |
| `/admin_stats` | Métricas operativas: usuarios registrados, activos, inactivos o expirados, cursos vigilados y cupo actual. |
| `/toggle_registro` | Abre o cierra el registro de nuevos usuarios sin reiniciar el bot. |
| `/set_max_users <n>` | Modifica el cupo máximo de estudiantes admitidos (`n > 0`). |

> **Nota:** Los intentos de invocar comandos de administración desde otro `chat_id` se ignoran en silencio para no revelar su existencia.

## Despliegue en producción (Systemd)

El despliegue en producción está estandarizado en `/opt/robertos-watching` bajo un usuario de sistema dedicado `robertos` y gestionado por `systemd` con directivas de endurecimiento del kernel de Linux.

### 1. Compilación optimizada del binario

En el entorno de compilación (VM OCI ARM64 o local x86_64 con `cargo zigbuild`):

```bash
cargo build --release
```

El perfil `[profile.release]` en `Cargo.toml` aplica optimizaciones completas (`opt-level = 3`), Link-Time Optimization (`lto = true`), una sola unidad de generación de código (`codegen-units = 1`) y eliminación de símbolos de depuración (`strip = true`). No se utiliza `panic = "abort"` para evitar que un pánico en una tarea aislada de Tokio tumbe el proceso entero.

### 2. Instalación con script idempotente

El repositorio incluye el script `deploy/install.sh` que automatiza la configuración completa del sistema:

```bash
sudo bash deploy/install.sh target/release/robertos-watching-bot
```

Este script ejecuta de manera idempotente:
1. Crea el usuario de sistema `robertos` (sin home y con `/usr/sbin/nologin`).
2. Crea el directorio `/opt/robertos-watching` (permisos `0750`) y `/opt/robertos-watching/backups` (permisos `0700`).
3. Copia el binario compilado hacia `/opt/robertos-watching/robertos-watching-bot` (permisos `0755`).
4. Instala las plantillas systemd en `/etc/systemd/system/`.
5. Instala `deploy/backup.sh` en `/opt/robertos-watching/backup.sh`.
6. Asegura permisos `0600` en `.env` y archivos de base de datos SQLite.
7. Recarga `systemd` (`daemon-reload`), habilita los servicios y reinicia el bot si `.env` está configurado.

### 3. Configuración del archivo `.env` de producción

En la máquina de producción, cree el archivo `/opt/robertos-watching/.env` con permisos estrictos `0600`:

```bash
sudo install -d -o robertos -g robertos -m 750 /opt/robertos-watching
sudo sh -c 'umask 077; cat > /opt/robertos-watching/.env' <<'EOF'
TELEGRAM_BOT_TOKEN=<TOKEN_PRODUCCION>
ADMIN_CHAT_ID=<TU_CHAT_ID>
DATABASE_URL=sqlite:///opt/robertos-watching/robertos_watching.db?mode=rwc
MAX_USERS_DEFAULT=300
BOT_TZ=America/Managua
RUST_LOG=info
SCAN_INTERVAL_MIN=30
EOF
sudo chown robertos:robertos /opt/robertos-watching/.env
sudo chmod 600 /opt/robertos-watching/.env
```

### 4. Gestión del servicio con systemd

```bash
# Iniciar o reiniciar el bot
sudo systemctl restart robertos-watching

# Consultar el estado del servicio
sudo systemctl status robertos-watching

# Inspeccionar logs en tiempo real
journalctl -u robertos-watching -f

# Detener el servicio limpiamente (systemd envía SIGTERM y el bot ejecuta shutdown ordenado)
sudo systemctl stop robertos-watching
```

### 5. Parámetros de endurecimiento del servicio (`robertos-watching.service`)

La unidad systemd (`deploy/robertos-watching.service`) incluye las siguientes directivas de seguridad:
- `User=robertos` y `Group=robertos`: Proceso ejecutado sin privilegios de root bajo usuario y grupo dedicados.
- `NoNewPrivileges=true`: Previene elevación de privilegios mediante `setuid`/`setgid`.
- `ProtectSystem=strict`: Monta todo el sistema de archivos (`/usr`, `/boot`, `/etc`) en modo solo lectura para el servicio.
- `ProtectHome=true`: Oculta `/home`, `/root` y `/run/user`.
- `PrivateTmp=true`: Asigna un `/tmp` y `/var/tmp` aislado e inaccesible para otros procesos.
- `ReadWritePaths=/opt/robertos-watching`: Única ruta del disco con permisos de escritura (para la base de datos SQLite en modo WAL y respaldos).
- `Restart=always` y `RestartSec=10`: Resiliencia continua ante caídas accidentales.
- `TimeoutStopSec=15`: Ventana adecuada para el cierre ordenado de conexiones activas.

## Estrategia de respaldo y recuperación

### Respaldo en caliente de SQLite (`deploy/backup.sh`)

SQLite en modo WAL no debe respaldarse mediante un simple comando `cp` mientras el bot está en ejecución, ya que copiar archivos WAL concurrentemente puede corromper el snapshot. 

El script `deploy/backup.sh` utiliza la API online de SQLite:
```bash
sqlite3 /opt/robertos-watching/robertos_watching.db ".backup '/opt/robertos-watching/backups/rw-<timestamp>.db'"
```

Características del respaldo:
- **Consistencia atómica**: Genera una copia válida sin detener el servicio ni bloquear lecturas o escrituras.
- **Resolución automática de configuración**: Detecta automáticamente `DATABASE_URL` y `BACKUP_RETENTION_DAYS` desde `/opt/robertos-watching/.env`, permitiendo también rutas personalizadas por argumento.
- **Verificación de integridad**: Ejecuta `PRAGMA integrity_check;` sobre el archivo resultante antes de considerarlo completado.
- **Permisos estrictos desde la creación**: Aplica `umask 077` durante todo el proceso, asegurando permisos `0600` y propiedad `robertos:robertos` en archivos de respaldo y `0700` en directorios.
- **Política de retención**: Purga automáticamente respaldos con más de 30 días de antigüedad (`BACKUP_RETENTION_DAYS=30`) y limpia archivos `.tmp` residuales.

### Automatización mediante systemd timer

El script `deploy/install.sh` instala y activa automáticamente:
- `robertos-watching-backup.service`: Ejecuta el respaldo como proceso *oneshot*.
- `robertos-watching-backup.timer`: Dispara el respaldo diariamente a las `03:00` con un retardo aleatorio de hasta 30 minutos (`RandomizedDelaySec=1800`) para mitigar picos de I/O.

Para verificar el estado del temporizador:
```bash
systemctl list-timers robertos-watching-backup.timer
```

### Procedimiento de recuperación ante desastre

Para restaurar la base de datos a partir de un respaldo:

```bash
# 1. Detener el bot
sudo systemctl stop robertos-watching

# 2. Restaurar la base de datos deseada
sudo -u robertos cp /opt/robertos-watching/backups/rw-<fecha_hora>.db /opt/robertos-watching/robertos_watching.db
sudo rm -f /opt/robertos-watching/robertos_watching.db-wal /opt/robertos-watching/robertos_watching.db-shm

# 3. Asegurar permisos 0600
sudo chmod 600 /opt/robertos-watching/robertos_watching.db
sudo chown robertos:robertos /opt/robertos-watching/robertos_watching.db

# 4. Iniciar el servicio
sudo systemctl start robertos-watching
```

## Troubleshooting y monitoreo

### Checklist de verificación operativa en producción

| Verificación | Comando | Resultado esperado |
| --- | --- | --- |
| **Arranque limpio** | `journalctl -u robertos-watching -n 50 --no-pager` | Inicialización de telemetría, base de datos WAL y workers sin errores ni pánicos. |
| **Consumo de memoria (RNF-01)** | `systemctl status robertos-watching` (línea `Memory:`) | Consumo menor a 30 MB RSS en reposo. |
| **Uso de CPU (RNF-02)** | `top -b -n1 -p $(pidof robertos-watching-bot)` | Consumo ≈ 0.0 % de CPU en reposo. |
| **Cero puertos de entrada (RNF-07)** | `sudo ss -tlnp \| grep robertos` | Salida vacía (el bot opera únicamente por polling HTTPS saliente hacia Telegram y Moodle). |
| **Permisos de archivos sensibles** | `stat -c '%a %U:%G' /opt/robertos-watching/.env /opt/robertos-watching/*.db` | `600 robertos:robertos`. |
| **Modo WAL en SQLite** | `sqlite3 /opt/robertos-watching/robertos_watching.db "PRAGMA journal_mode;"` | `wal`. |
| **Sin secretos en logs** | `journalctl -u robertos-watching --no-pager \| grep -ciE "password\|wstoken"` | `0` coincidencias. |

### Diagnóstico de incidencias frecuentes

1. **El servicio falla al iniciar (`systemd status` muestra código de salida distinto de 0):**
   - Ejecute `journalctl -u robertos-watching -e --no-pager` para ver el error exacto.
   - Si indica `ConfigError::MissingVar`, revise que todas las variables requeridas existan en `/opt/robertos-watching/.env`.
   - Si indica `ConfigError::InvalidDatabaseUrl`, asegúrese de usar triple barra: `sqlite:///opt/robertos-watching/...`.
   - Si indica permisos denegados al abrir la base de datos, ejecute: `sudo chown -R robertos:robertos /opt/robertos-watching && sudo chmod 600 /opt/robertos-watching/*.db*`.

2. **Caídas o mantenimiento en Moodle:**
   - Roberto's Watching incluye un *Circuit Breaker* integrado que detecta respuestas erróneas consecutivas (HTTP 503, timeouts o caídas de red).
   - El bot protege al servidor remoto reduciendo el tráfico con backoff exponencial y emite una alerta al `ADMIN_CHAT_ID`.
   - Al restablecerse la conectividad, el bot reanuda el escaneo normal e informa la recuperación al administrador.

3. **Notificación de token expirado:**
   - Si la plataforma de Moodle invalida un token de sesión o el estudiante cambia su contraseña, el bot detecta el código de error tipado de Moodle, marca la cuenta como inactiva (`is_active = 0`, `wstoken = ''`) y le notifica en privado que debe reautenticarse con `/login`.

## Buenas prácticas de despliegue

- Genere el token del bot con @BotFather y rótelo con `/revoke` si se sospecha de su exposición.
- Mantenga `.env` con permisos `0600` y propiedad del usuario de servicio; no lo incluya en imágenes de contenedor ni en copias de seguridad sin cifrar.
- Ejecute el proceso con un usuario sin privilegios, con el directorio de datos como único lugar con escritura.
- Aplique permisos `0600` al archivo SQLite (y a sus `-wal`/`-shm`), y cifre el volumen donde resida.
- Use `RUST_LOG=info` en producción; reserve `debug` para diagnósticos temporales y revise los logs antes de compartirlos.
- Mantenga las dependencias al día y audítelas periódicamente con `cargo audit` o `cargo deny`.
- Respalde la base de datos de forma cifrada y defina una política de retención acorde al propósito del servicio.
- Informe a los usuarios qué datos se almacenan (identificador de Telegram, identificador de usuario de Moodle, nombre, cursos, tareas y entregas) y que `/logout` los elimina.