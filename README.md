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
- [Desarrollo y pruebas](#desarrollo-y-pruebas)
- [Seguridad](#seguridad)
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

Comandos de administración (solo `ADMIN_CHAT_ID`): `/admin_stats`, `/toggle_registro`, `/set_max_users <n>`.

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

## Buenas prácticas de despliegue

- Genere el token del bot con @BotFather y rótelo con `/revoke` si se sospecha de su exposición.
- Mantenga `.env` con permisos `0600` y propiedad del usuario de servicio; no lo incluya en imágenes de contenedor ni en copias de seguridad sin cifrar.
- Ejecute el proceso con un usuario sin privilegios, con el directorio de datos como único lugar con escritura.
- Aplique permisos `0600` al archivo SQLite (y a sus `-wal`/`-shm`), y cifre el volumen donde resida.
- Use `RUST_LOG=info` en producción; reserve `debug` para diagnósticos temporales y revise los logs antes de compartirlos.
- Mantenga las dependencias al día y audítelas periódicamente con `cargo audit` o `cargo deny`.
- Respalde la base de datos de forma cifrada y defina una política de retención acorde al propósito del servicio.
- Informe a los usuarios qué datos se almacenan (identificador de Telegram, identificador de usuario de Moodle, nombre, cursos, tareas y entregas) y que `/logout` los elimina.