#!/usr/bin/env bash
# ==============================================================================
# Roberto's Watching — Idempotent Installation and Update Script
# ==============================================================================
# Instala o actualiza el servicio del bot y sus componentes auxiliares en /opt/robertos-watching:
# 1. Crea el usuario de sistema no privilegiado 'robertos' si no existe.
# 2. Crea y asegura los directorios de trabajo y respaldos con permisos estrictos.
# 3. Instala el binario compilado (target/release/robertos-watching-bot).
# 4. Instala las plantillas systemd (servicio principal, timer y servicio de respaldo).
# 5. Instala el script de respaldo online de SQLite.
# 6. Aplica permisos 0600 a archivos sensibles (.env y bases de datos).
# 7. Recarga systemd, habilita los servicios y reinicia si la configuración existe.
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

DESTDIR="${DESTDIR:-}"
INSTALL_DIR="${DESTDIR}/opt/robertos-watching"
BACKUP_DIR="${INSTALL_DIR}/backups"
SYSTEMD_DIR="${DESTDIR}/etc/systemd/system"
SERVICE_NAME="robertos-watching.service"
BACKUP_SERVICE_NAME="robertos-watching-backup.service"
BACKUP_TIMER_NAME="robertos-watching-backup.timer"
SYSTEM_USER="robertos"

log() {
    printf "[%s] [INSTALL] %s\n" "$(date '+%Y-%m-%d %H:%M:%S')" "$*"
}

err() {
    printf "[%s] [INSTALL-ERROR] %s\n" "$(date '+%Y-%m-%d %H:%M:%S')" "$*" >&2
}

# 1. Comprobar ejecución con permisos de root (si no se especifica DESTDIR)
if [[ -z "$DESTDIR" && "$(id -u)" -ne 0 ]]; then
    err "Este script requiere privilegios de root para administrar usuarios y unidades systemd."
    err "Ejecute: sudo bash $0 [ruta_al_binario]"
    exit 1
fi

# Validación temprana de argumentos
BINARY_INPUT="${1:-}"
if [[ -n "$BINARY_INPUT" && ! -f "$BINARY_INPUT" ]]; then
    err "El archivo binario especificado '$BINARY_INPUT' no existe o no es un archivo regular."
    exit 2
fi

log "Iniciando instalación/actualización idempotente de Roberto's Watching..."

# 2. Comprobar disponibilidad de sqlite3 para respaldos
if ! command -v sqlite3 >/dev/null 2>&1; then
    log "Comando 'sqlite3' no encontrado en el sistema."
    if [[ "$(id -u)" -eq 0 ]] && command -v apt-get >/dev/null 2>&1; then
        log "Instalando sqlite3 mediante apt-get..."
        apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq sqlite3 || true
    else
        err "AVISO: 'sqlite3' no está instalado. Se requiere para el script de respaldos (deploy/backup.sh)."
    fi
fi

# 3. Crear grupo y usuario de sistema no privilegiado si no existen
if [[ -z "$DESTDIR" ]]; then
    if ! getent group "$SYSTEM_USER" >/dev/null 2>&1; then
        log "Creando grupo de sistema '$SYSTEM_USER'..."
        groupadd --system "$SYSTEM_USER"
    fi

    if ! id -u "$SYSTEM_USER" >/dev/null 2>&1; then
        log "Creando usuario de sistema '$SYSTEM_USER'..."
        useradd --system -g "$SYSTEM_USER" --no-create-home --shell /usr/sbin/nologin \
            --comment "Roberto's Watching Service User" "$SYSTEM_USER"
    else
        log "Usuario de sistema '$SYSTEM_USER' ya existe."
    fi
fi

# 4. Crear directorios de ejecución y respaldos
log "Configurando directorios en '$INSTALL_DIR'..."
if [[ -z "$DESTDIR" && "$(id -u)" -eq 0 ]]; then
    install -d -m 750 -o "$SYSTEM_USER" -g "$SYSTEM_USER" "$INSTALL_DIR"
    install -d -m 700 -o "$SYSTEM_USER" -g "$SYSTEM_USER" "$BACKUP_DIR"
else
    mkdir -p "$INSTALL_DIR" "$BACKUP_DIR"
    chmod 750 "$INSTALL_DIR"
    chmod 700 "$BACKUP_DIR"
fi

# 5. Localizar e instalar binario
BINARY_SRC=""

if [[ -n "$BINARY_INPUT" ]]; then
    BINARY_SRC="$BINARY_INPUT"
elif [[ -f "${REPO_DIR}/target/release/robertos-watching-bot" ]]; then
    BINARY_SRC="${REPO_DIR}/target/release/robertos-watching-bot"
elif [[ -f "${REPO_DIR}/target/x86_64-unknown-linux-gnu/release/robertos-watching-bot" ]]; then
    BINARY_SRC="${REPO_DIR}/target/x86_64-unknown-linux-gnu/release/robertos-watching-bot"
elif [[ -f "${REPO_DIR}/target/aarch64-unknown-linux-gnu/release/robertos-watching-bot" ]]; then
    BINARY_SRC="${REPO_DIR}/target/aarch64-unknown-linux-gnu/release/robertos-watching-bot"
elif [[ -n "${SUDO_USER:-}" && -f "/home/${SUDO_USER}/robertos-watching-bot" ]]; then
    BINARY_SRC="/home/${SUDO_USER}/robertos-watching-bot"
elif [[ -f "${HOME}/robertos-watching-bot" ]]; then
    BINARY_SRC="${HOME}/robertos-watching-bot"
elif [[ -f "./robertos-watching-bot" ]]; then
    BINARY_SRC="./robertos-watching-bot"
fi

if [[ -n "$BINARY_SRC" ]]; then
    log "Instalando binario desde '$BINARY_SRC' hacia '${INSTALL_DIR}/robertos-watching-bot'..."
    if [[ -z "$DESTDIR" && "$(id -u)" -eq 0 ]]; then
        install -m 755 -o "$SYSTEM_USER" -g "$SYSTEM_USER" "$BINARY_SRC" "${INSTALL_DIR}/robertos-watching-bot"
    else
        install -m 755 "$BINARY_SRC" "${INSTALL_DIR}/robertos-watching-bot"
    fi
elif [[ -f "${INSTALL_DIR}/robertos-watching-bot" ]]; then
    log "No se proporcionó un nuevo binario; se conserva el binario existente en '${INSTALL_DIR}/robertos-watching-bot'."
else
    err "No se encontró el binario compilado 'robertos-watching-bot'."
    err "Compile el proyecto con 'cargo build --release' o pase la ruta al binario: sudo bash $0 <ruta_al_binario>"
    exit 2
fi

# 6. Instalar scripts auxiliares
log "Instalando script de respaldo en '${INSTALL_DIR}/backup.sh'..."
if [[ -z "$DESTDIR" && "$(id -u)" -eq 0 ]]; then
    install -m 750 -o "$SYSTEM_USER" -g "$SYSTEM_USER" "${SCRIPT_DIR}/backup.sh" "${INSTALL_DIR}/backup.sh"
    if [[ -f "${REPO_DIR}/.env.example" ]]; then
        install -m 644 -o "$SYSTEM_USER" -g "$SYSTEM_USER" "${REPO_DIR}/.env.example" "${INSTALL_DIR}/.env.example"
    fi
else
    install -m 750 "${SCRIPT_DIR}/backup.sh" "${INSTALL_DIR}/backup.sh"
    if [[ -f "${REPO_DIR}/.env.example" ]]; then
        install -m 644 "${REPO_DIR}/.env.example" "${INSTALL_DIR}/.env.example"
    fi
fi

# 7. Validar y asegurar permisos en .env
if [[ -f "${INSTALL_DIR}/.env" ]]; then
    log "Asegurando permisos 0600 en '${INSTALL_DIR}/.env'..."
    if [[ -z "$DESTDIR" && "$(id -u)" -eq 0 ]]; then
        chown "${SYSTEM_USER}:${SYSTEM_USER}" "${INSTALL_DIR}/.env"
    fi
    chmod 600 "${INSTALL_DIR}/.env"
else
    log "AVISO: '${INSTALL_DIR}/.env' no existe aún."
    log "Copie '${INSTALL_DIR}/.env.example' a '${INSTALL_DIR}/.env', configure sus credenciales y ejecute: chmod 600 ${INSTALL_DIR}/.env"
fi

# 8. Asegurar permisos en bases de datos SQLite y respaldos si ya existen
shopt -s nullglob
DB_FILES=("${INSTALL_DIR}"/*.db* "${BACKUP_DIR}"/*.db*)
shopt -u nullglob

if [[ ${#DB_FILES[@]} -gt 0 ]]; then
    log "Asegurando permisos 0600 en archivos de base de datos..."
    for f in "${DB_FILES[@]}"; do
        if [[ -f "$f" ]]; then
            if [[ -z "$DESTDIR" && "$(id -u)" -eq 0 ]]; then
                chown "${SYSTEM_USER}:${SYSTEM_USER}" "$f"
            fi
            chmod 600 "$f"
        fi
    done
fi

# 9. Instalar unidades systemd
log "Instalando unidades systemd en '${SYSTEMD_DIR}'..."
mkdir -p "$SYSTEMD_DIR"
install -m 644 "${SCRIPT_DIR}/${SERVICE_NAME}" "${SYSTEMD_DIR}/${SERVICE_NAME}"
install -m 644 "${SCRIPT_DIR}/${BACKUP_SERVICE_NAME}" "${SYSTEMD_DIR}/${BACKUP_SERVICE_NAME}"
install -m 644 "${SCRIPT_DIR}/${BACKUP_TIMER_NAME}" "${SYSTEMD_DIR}/${BACKUP_TIMER_NAME}"

# 10. Recargar systemd y habilitar servicios (si no estamos en entorno DESTDIR)
if [[ -z "$DESTDIR" ]]; then
    log "Recargando demonio systemd y habilitando unidades..."
    systemctl daemon-reload
    systemctl enable "$SERVICE_NAME"
    systemctl enable --now "$BACKUP_TIMER_NAME"
    systemctl restart "$BACKUP_TIMER_NAME" || true

    # Iniciar o reiniciar servicio si .env está presente
    if [[ -f "${INSTALL_DIR}/.env" ]]; then
        log "Reiniciando '${SERVICE_NAME}' para aplicar cambios..."
        if ! systemctl restart "$SERVICE_NAME"; then
            err "Fallo al iniciar o reiniciar '${SERVICE_NAME}'."
            systemctl status "$SERVICE_NAME" --no-pager || true
            err "Consulte los logs detallados con: journalctl -u ${SERVICE_NAME} -n 50 --no-pager"
            exit 1
        fi
        log "Estado actual del servicio:"
        systemctl status "$SERVICE_NAME" --no-pager || true
    else
        log "Instalación completada. Cree '${INSTALL_DIR}/.env' y arranque con: systemctl start ${SERVICE_NAME}"
    fi
else
    log "DESTDIR activo ('$DESTDIR'): instalación de archivos completada sin invocar systemctl."
fi

log "¡Instalación completada exitosamente!"
