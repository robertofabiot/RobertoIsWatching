<p align="center">
  <img src="assets/logo-rounded.svg" alt="Logo de Roberto's Watching" width="180">
</p>

<h1 align="center">Roberto's Watching</h1>

<p align="center">
  Tu asistente académico en Telegram: vigila tus cursos de Moodle para que no se te pase ninguna entrega.
</p>

<p align="center">
  <a href="https://t.me/roberto_is_watching_bot"><img src="https://img.shields.io/badge/Abrir%20en-Telegram-26A5E4?logo=telegram&logoColor=white" alt="Abrir en Telegram"></a>
  <img src="https://img.shields.io/badge/estado-en%20desarrollo-yellow" alt="Estado: en desarrollo">
  <img src="https://img.shields.io/badge/Rust-stable-orange?logo=rust" alt="Rust">
  <img src="https://img.shields.io/badge/SQLite-003B57?logo=sqlite&logoColor=white" alt="SQLite">
</p>

---

## De qué trata

Revisar la plataforma de la universidad todos los días para ver si hay una tarea nueva, una nota publicada o un plazo a punto de vencer es tedioso, y es fácil olvidarlo. Roberto's Watching lo hace por ti: se conecta a tus cursos de Moodle, los revisa de forma periódica y te escribe por Telegram cuando hay algo que merece tu atención.

## Cómo funciona

1. **Vincula tu cuenta.** Inicias sesión una sola vez desde el chat privado con el bot.
2. **El bot vigila por ti.** Revisa tus cursos cada cierto tiempo y detecta tareas nuevas, cambios en las entregas y calificaciones publicadas.
3. **Recibes avisos.** Te llega una alerta antes de que venza un plazo y un resumen de tu día a las horas que tú elijas.
4. **Consultas cuando quieras.** Con unos pocos comandos ves tus pendientes, tus notas y tus cursos al instante.

## Qué ofrece

- **Alertas de plazos:** un aviso antes de cada vencimiento, sin repetir notificaciones.
- **Resúmenes programados:** tu situación académica del día, en los horarios y días que configures.
- **Tareas pendientes de un vistazo:** qué debes entregar y para cuándo.
- **Calificaciones y retroalimentación:** te enteras de las notas nuevas sin entrar a la plataforma.
- **Sincronización a demanda:** si no quieres esperar al siguiente chequeo, la fuerzas con `/sync`.
- **Tu información, bajo tu control:** con `/logout` se eliminan tus datos.

## Empieza a usarlo

El bot ya está en funcionamiento: [@roberto_is_watching_bot](https://t.me/roberto_is_watching_bot).

1. Abre el enlace y pulsa **Iniciar**.
2. Escribe `/login <usuario> <contraseña>` en el chat privado con el bot.
3. Prueba `/pendientes`, `/notas` o `/resumen`.

Al ser un proyecto en desarrollo, el comportamiento y los comandos pueden cambiar.

## Comandos

| Comando | Qué hace |
| --- | --- |
| `/start` | Mensaje de bienvenida. |
| `/ayuda` | Lista de comandos disponibles. |
| `/login <usuario> <contraseña>` | Vincula tu cuenta de Moodle (solo en chat privado). |
| `/cursos` | Muestra los cursos que el bot vigila. |
| `/pendientes` | Entregas y tareas por hacer. |
| `/notas` | Calificaciones y retroalimentación reciente. |
| `/resumen` | Resumen académico del día. |
| `/sync` | Actualiza tu información ahora (con una pausa entre usos). |
| `/config` | Elige las horas y los días de tus resúmenes. |
| `/logout` | Cierra tu sesión y elimina tus datos. |

## Privacidad y seguridad

Se diseñó pensando en que manejas credenciales sensibles:

- **Tu contraseña no se guarda.** Se usa una sola vez para iniciar sesión, se borra de la memoria y no aparece en los registros. El mensaje de `/login` se elimina del chat automáticamente.
- **Solo conversaciones privadas.** El inicio de sesión se rechaza en grupos y canales.
- **Protección contra abusos.** Hay un límite de intentos fallidos de inicio de sesión para que el bot no pueda usarse para adivinar contraseñas ajenas.
- **Tus datos se pueden borrar.** `/logout` elimina tu información, tus alertas y tus avisos programados.
- **Comunicaciones cifradas.** Toda la comunicación con Telegram y con Moodle va sobre TLS.
- **Respeto por el servidor de Moodle.** El bot limita sus consultas para no sobrecargar la plataforma y se pausa solo si esta deja de responder.

Conviene saber dos cosas. Tras iniciar sesión, el bot conserva un token de acceso de Moodle (no tu contraseña) en su base de datos, y el despliegue recomendado lo protege con permisos de archivo y cifrado de disco. Además, Telegram no cifra de extremo a extremo los chats con bots, por eso el mensaje de `/login` se elimina de inmediato.

Para reportar una vulnerabilidad, usa el aviso privado de seguridad de GitHub (Security > Report a vulnerability) en lugar de un issue público.

## Tecnología

Escrito en Rust con [teloxide](https://github.com/teloxide/teloxide) para Telegram, `tokio` como motor asíncrono y SQLite para guardar el estado. Se comunica con Moodle a través de su API de servicios web. El código está organizado en módulos para el bot, el cliente de Moodle, los servicios de sincronización y alertas, el planificador de tareas y el almacenamiento.

## Para quien quiera ejecutar su propia instancia

Necesitas Rust estable, un token de [@BotFather](https://t.me/BotFather) y tu ID numérico de Telegram (por ejemplo con [@userinfobot](https://t.me/userinfobot)).

```bash
git clone https://github.com/robertofabiot/RobertoIsWatching.git
cd RobertoIsWatching
cp .env.example .env
chmod 600 .env
# edita .env con tus valores
cargo run --release
```

Variables principales de configuración:

| Variable | Descripción |
| --- | --- |
| `TELEGRAM_BOT_TOKEN` | Token del bot emitido por @BotFather. |
| `ADMIN_CHAT_ID` | ID de Telegram de la persona administradora. |
| `DATABASE_URL` | Ubicación de la base de datos SQLite. |
| `BOT_TZ` | Zona horaria de los resúmenes, por ejemplo `America/Managua`. |
| `MAX_USERS_DEFAULT` | Cupo máximo de usuarios (opcional, 300 por defecto). |
| `SCAN_INTERVAL_MIN` | Minutos entre revisiones de Moodle (opcional, 30 por defecto). |

Para desplegar en producción con systemd, respaldos automáticos y guía de diagnóstico, consulta la [guía de despliegue y operación](DEPLOYMENT.md). Para ejecutar las pruebas:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```
