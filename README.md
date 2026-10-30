# Roberto's Watching

Bot de Telegram que vigila de forma continua los cursos de un estudiante en Moodle y le avisa de nuevas tareas, entregas pendientes, calificaciones y plazos próximos a vencer, además de enviarle resúmenes académicos programados.

![Estado](https://img.shields.io/badge/estado-en%20desarrollo-yellow)
![Rust](https://img.shields.io/badge/Rust-stable-orange?logo=rust)
![Edition](https://img.shields.io/badge/edition-2021-blue)
![Telegram](https://img.shields.io/badge/Telegram-teloxide%200.17-26A5E4?logo=telegram&logoColor=white)
![SQLite](https://img.shields.io/badge/SQLite-sqlx%200.8-003B57?logo=sqlite&logoColor=white)
![TLS](https://img.shields.io/badge/TLS-rustls%20(sin%20OpenSSL)-success)
![Clippy](https://img.shields.io/badge/clippy-warnings%20denied-informational)

## Tabla de contenidos

- [Acceso al bot](#acceso-al-bot)
- [Características](#características)
- [Arquitectura](#arquitectura)
- [Requisitos](#requisitos)
- [Instalación y ejecución](#instalación-y-ejecución)
- [Configuración](#configuración)
- [Comandos del bot](#comandos-del-bot)
- [Despliegue en producción (Systemd)](#despliegue-en-producción-systemd)
- [Estrategia de respaldo y recuperación](#estrategia-de-respaldo-y-recuperación)
- [Desarrollo y pruebas](#desarrollo-y-pruebas)
- [Seguridad](#seguridad)
- [Troubleshooting y monitoreo](#troubleshooting-y-monitoreo)
- [Buenas prácticas de despliegue](#buenas-prácticas-de-despliegue)

## Acceso al bot

El bot se encuentra en funcionamiento y puede usarse directamente desde Telegram: [@roberto_is_watching_bot](https://t.me/roberto_is_watching_bot).

1. Abra el enlace y pulse **Iniciar** (`/start`).
2. Vincule su cuenta con `/login <usuario> <contraseña>` en el chat privado con el bot.
3. Use `/pendientes`, `/notas` o `/resumen` para consultar su información. Con `/logout` puede eliminar sus datos en cualquier momento.

El proyecto está en desarrollo activo, por lo que el comportamiento y los comandos pueden cambiar.

## Características

- Sincronización periódica de cursos, tareas y estado de entregas mediante la API de web services de Moodle.
- Alertas de plazos próximos a vencer, con deduplicación para no notificar dos veces el mismo evento.
- Resúmenes académicos por usuario, con horarios y días configurables, alineados al minuto en la zona horaria del bot.
- Escaneo compartido entre usuarios: los cursos comunes se consultan una sola vez por ciclo.
- Control de admisión con cupo máximo de usuarios y comandos de administración.
- Resiliencia frente a caídas del servidor remoto: circuit breaker, concurrencia acotada y jitter.
- Apagado ordenado con cancelación de workers.

## Arquitectura

| Módulo | Responsabilidad |
| --- | --- |
| `bot` | Dispatcher de Telegram, handlers de comandos, plantillas de mensajes y limitador de tasa. |
| `moodle` | Cliente HTTP de Moodle: autenticación, decodificación de respuestas, gate de concurrencia de dos niveles, circuit breaker y métricas de peticiones. |
| `services` | Lógica de aplicación: sincronización, escáner periódico, alertas y sesión. |
| `scheduler` | Temporizadores de plazos y worker de resúmenes programados. |
| `storage` | Repositorios sobre SQLite (`sqlx`), modelos y migraciones embebidas. |
| `config` | Carga y validación estricta de variables de entorno. |
| `telemetry` | Inicialización de `tracing` y filtrado por `RUST_LOG`. |

Stack: Rust (edición 2021) sobre `tokio`, `teloxide` para Telegram, `reqwest` con `rustls` para HTTP, `sqlx` con SQLite para persistencia y `chrono-tz` para el manejo de zonas horarias.

## Requisitos

- Toolchain de Rust estable (`rust-toolchain.toml` instala `rustfmt` y `clippy` automáticamente con `rustup`).
- Un token de bot emitido por [@BotFather](https://t.me/BotFather).
- El ID numérico de Telegram de la persona administradora (por ejemplo, mediante [@userinfobot](https://t.me/userinfobot)).
- Acceso de red saliente hacia Telegram y hacia el servidor Moodle objetivo.

No se requieren bibliotecas del sistema como OpenSSL: toda la pila TLS es `rustls`.

## Instalación y ejecución

```bash
git clone https://github.com/robertofabiot/RobertoIsWatching.git
cd RobertoIsWatching
cp .env.example .env
chmod 600 .env
```

Edite `.env` con sus valores (ver [Configuración](#configuración)) y ejecute:

```bash
cargo run --release
```

Las migraciones de `migrations/` se aplican automáticamente al arrancar; con `mode=rwc` en `DATABASE_URL` la base de datos SQLite se crea si no existe.

## Configuración

Toda la configuración se realiza mediante variables de entorno (o un archivo `.env`). El arranque falla de forma explícita si algún valor requerido falta o es inválido.

| Variable | Requerida | Por defecto | Descripción |
| --- | --- | --- | --- |
| `TELEGRAM_BOT_TOKEN` | Sí | n/a | Token del bot emitido por @BotFather. |
| `ADMIN_CHAT_ID` | Sí | n/a | ID numérico de Telegram con acceso a los comandos de administración. |
| `DATABASE_URL` | Sí | n/a | URL de conexión SQLite, por ejemplo `sqlite://robertos_watching.db?mode=rwc`. |
| `BOT_TZ` | Sí | n/a | Zona horaria IANA para resúmenes y cálculo de plazos, por ejemplo `America/Managua`. |
| `MAX_USERS_DEFAULT` | No | `300` | Cupo máximo inicial de usuarios registrados. |
| `SCAN_INTERVAL_MIN` | No | `30` | Intervalo del escaneo periódico en minutos (rango 1 a 1440). |
| `RUST_LOG` | No | n/a | Filtro de `tracing`, por ejemplo `info,robertos_watching_bot=info`. |

## Comandos del bot

Comandos para usuarios:

| Comando | Descripción |
| --- | --- |
| `/start` | Mensaje de bienvenida. |
| `/ayuda` | Catálogo de comandos. |
| `/login <usuario> <contraseña>` | Vincula la cuenta de Moodle (solo en chat privado). |
| `/cursos` | Lista de cursos vigilados. |
| `/pendientes` | Entregas y tareas pendientes. |
| `/notas` | Calificaciones y retroalimentación reciente. |
| `/resumen` | Resumen académico del día. |
| `/sync` | Fuerza una sincronización (con tiempo de espera entre usos). |
| `/config` | Ajusta horarios y días de los resúmenes. |
| `/logout` | Cierra la sesión y elimina los datos del usuario. |

### Comandos de administración (solo `ADMIN_CHAT_ID`)

| Comando | Descripción |
| --- | --- |
| `/admin_stats` | Consulta métricas operativas en tiempo real: usuarios registrados, activos, inactivos/expirados, cursos vigilados y cupo actual. |
| `/toggle_registro` | Abre o cierra el registro de nuevos usuarios en caliente sin reiniciar el bot. |
| `/set_max_users <n>` | Modifica dinámicamente el cupo máximo de estudiantes admitidos (`n > 0`). |

> **Nota:** Cualquier intento de invocar comandos de administración desde un `chat_id` distinto de `ADMIN_CHAT_ID` es ignorado en silencio para no revelar la existencia de la superficie de administración.

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
- `User=robertos`: Proceso ejecutado sin privilegios de root.
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
- **Verificación de integridad**: Ejecuta `PRAGMA integrity_check;` sobre el archivo resultante antes de considerarlo completado.
- **Permisos estrictos**: Asigna permisos `0600` y propiedad `robertos:robertos`.
- **Política de retención**: Purga automáticamente respaldos con más de 30 días de antigüedad (`BACKUP_RETENTION_DAYS=30`).

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

## Desarrollo y pruebas

La suite incluye pruebas unitarias y de integración sobre SQLite, además de pruebas contra un servidor Moodle simulado con `wiremock` y fixtures anonimizados.

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

El pipeline de CI (`.github/workflows/ci.yml`) ejecuta estas mismas verificaciones en cada push y pull request hacia `main`, y además comprueba que `openssl-sys` no aparezca en el grafo de dependencias.

## Seguridad

Controles implementados en el código:

- **Manejo de contraseñas.** El mensaje de `/login` se elimina de Telegram como primera instrucción del handler. La contraseña se envuelve en un buffer `Zeroizing` y se borra de memoria al recibir la respuesta de Moodle. Nunca se persiste ni se escribe en logs.
- **Solo chats privados.** `/login` se rechaza en grupos y canales.
- **Anti fuerza bruta.** Máximo 3 intentos fallidos por chat cada 10 minutos, para que el bot no pueda usarse como oráculo contra cuentas de terceros.
- **Credenciales no persistentes.** Tras autenticar, solo se conserva el token de web service de Moodle; el usuario puede revocar su presencia con `/logout`, que elimina sus datos y alertas y cancela sus tareas programadas.
- **Secretos fuera de los logs.** `Debug` de `Config` enmascara el token del bot, y existen pruebas que verifican que el tracing del cliente HTTP no filtre tokens ni credenciales.
- **Fixtures sin datos reales.** Una prueba de higiene verifica que los fixtures de `tests/fixtures` no contengan información sensible.
- **Superficie de ataque reducida.** TLS con `rustls` (sin OpenSSL), cliente HTTP sin almacén de cookies y comandos de administración restringidos a `ADMIN_CHAT_ID`.
- **Protección del servidor remoto y del propio bot.** Concurrencia limitada con prioridades, jitter, circuit breaker y cupo máximo de usuarios.
- **Consultas parametrizadas.** Todo acceso a la base de datos usa `sqlx` con parámetros enlazados.
- **Secretos fuera del repositorio.** `.env`, bases de datos y logs están en `.gitignore`; el repositorio solo contiene `.env.example` con valores ficticios.

Para reportar una vulnerabilidad, abra un aviso privado de seguridad en GitHub (Security > Report a vulnerability) en lugar de un issue público.

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

2. **Caídas o mantenimiento en Moodle (UAM Virtual):**
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