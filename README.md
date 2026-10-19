# omni-term

Terminal multiplataforma en Rust nativo con una interfaz pensada como un IDE: terminal local, conexiones SSH guardadas, explorador SFTP, túneles y snippets.

## Qué incluye

- **Terminal local y SSH nativo** en pestañas, con la sesión restaurada al reabrir. Autenticación con contraseña, llave (con passphrase), `ssh-agent` e interactiva (2FA). Saltos entre servidores (`ProxyJump`) y verificación de huellas en `~/.ssh/known_hosts`.
- **Identidad por conexión**: cada conexión puede llevar un color y un entorno («producción», «staging»). Se ve en la barra lateral, en la pestaña, en una banda bajo las pestañas y en el título de la ventana, para no escribir en el servidor equivocado.
- **Secretos protegidos**: keyring del sistema, o una clave maestra propia (Argon2id + XChaCha20-Poly1305) si no hay keyring.
- **Túneles** guardados por conexión: `-L`, `-R` y `-D` (proxy SOCKS5), con su estado en la barra lateral.
- **Explorador SFTP** de dos paneles (local y remoto): arrastrar y soltar, transferencias recursivas en segundo plano con cancelación, resolución de conflictos, permisos y papelera local.
- **Snippets** con búsqueda y variables `{{nombre}}` / `{{nombre=defecto}}`.
- **Paneles divididos** dentro de cada pestaña, con divisores arrastrables. Una pestaña SSH divide abriendo otra sesión a la misma conexión.
- **Terminal completo**: ratón para `vim`/`tmux`/`htop`, protocolo de teclado kitty, búsqueda en el historial, enlaces clicables, color verdadero y bloques/cajas dibujados como geometría.
- Tema oscuro, claro o del sistema, tamaño de fuente, cursor y tamaño de ventana recordados.

## Atajos

| Atajo | Acción |
|---|---|
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copiar la selección / pegar |
| `Ctrl+Shift+F` | Buscar en el historial (`Enter` siguiente, `Mayús+Enter` anterior, `Esc` cierra) |
| `Ctrl+Shift+D` / `Ctrl+Shift+E` | Dividir el panel a la derecha / hacia abajo (también en el menú *Dividir*) |
| `Ctrl+Shift+W` | Cerrar el panel con foco (o la pestaña si es el único) |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | Ampliar, reducir y restablecer la fuente |
| `Ctrl` + clic | Abrir el enlace bajo el puntero |
| `Mayús` + arrastrar | Seleccionar texto aunque la aplicación use el ratón |

`Ctrl+C` se envía siempre a la aplicación (interrumpe el proceso).

## Instalar

Descarga el archivo de tu sistema desde las *Releases*. En Linux, el paquete incluye `install.sh`, que instala el binario, el icono y la entrada de escritorio para tu usuario:

```bash
./install.sh
```

También puedes compilarlo:

```bash
cargo build --release
packaging/install-linux.sh        # opcional, solo Linux
```

En Linux necesitas las cabeceras de desarrollo de `libxkbcommon`, `wayland`, `x11` y `dbus`.

## Datos

La base de datos (conexiones, túneles, snippets, sesión y ajustes) está en el directorio de datos del sistema (`~/.local/share/omniterm/omniterm.db` en Linux). Las contraseñas no se guardan ahí, salvo cifradas con la clave maestra.

## Desarrollo

```bash
cargo test            # incluye pruebas de extremo a extremo con un sshd privado en Linux
cargo run -- --keys   # imprime las teclas recibidas y los bytes enviados
```

## Versiones

omni-term sigue [versionado semántico](https://semver.org/lang/es/) (`MAYOR.MENOR.PARCHE`) y anota los cambios en [CHANGELOG.md](CHANGELOG.md). Mientras la versión mayor sea 0, los cambios incompatibles suben la menor.

Las versiones son automáticas. Cada commit a `master` con [commits convencionales](https://www.conventionalcommits.org/es/) (`fix:` sube el parche, `feat:` la menor, `!` o `BREAKING CHANGE` la mayor) hace que release-plz abra o actualice un PR «chore(release)» con la versión nueva y el CHANGELOG. Al fusionarlo se crea la etiqueta `vX.Y.Z`, y esa etiqueta compila y publica los binarios de Linux, Windows y macOS.

Requisito único: un secreto `RELEASE_PLZ_TOKEN` en el repositorio, con un token personal con permisos de contenido y pull requests. Con el `GITHUB_TOKEN` por defecto la etiqueta no dispararía el flujo de publicación.

`omni-term --version` muestra la versión instalada. El flujo de publicación falla si la etiqueta no coincide con `Cargo.toml`.
