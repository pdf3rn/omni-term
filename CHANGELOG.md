# Changelog

Todos los cambios notables de omni-term. A partir de la 0.1.0 este archivo lo actualiza release-plz a partir de los commits convencionales.
El formato sigue [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/) y el proyecto usa [versionado semántico](https://semver.org/lang/es/).

## [0.1.5](https://github.com/pdf3rn/omni-term/compare/v0.1.4...v0.1.5) - 2026-10-31

### Added

- *(containers)* reach remote docker and podman engines over ssh
- *(macos)* add native menu bar and fullsize titlebar
- *(containers)* detect podman machine socket on macos
- *(containers)* support docker and podman named pipes on windows
- *(ui)* make dialog submit keys explicit per spec
- *(terminal)* enable ime input and route ime commits

### Fixed

- *(ui)* add focus flag to unlock prompt and fix status margin
- *(terminal)* close pty when shell exits so tab closes on windows
- *(terminal)* send each pasted line break once as carriage return
- *(main)* prefer dx12 backend on windows to avoid window flicker
- *(ui)* ask before pasting multi-line text into the terminal
- *(smb)* release client inside tokio runtime on drop

### Other

- *(release)* build macos dmg with applications shortcut
- *(release)* ad-hoc sign macos app bundle
- *(deps)* bump the cargo-minor-and-patch group with 3 updates ([#12](https://github.com/pdf3rn/omni-term/pull/12))
- *(deps)* bump softprops/action-gh-release from 2 to 3 ([#11](https://github.com/pdf3rn/omni-term/pull/11))

## [0.1.4](https://github.com/pdf3rn/omni-term/compare/v0.1.3...v0.1.4) - 2026-10-24

### Added

- *(local)* add configurable local terminal profiles
- *(dialogs)* replace the connection type rail with a grouped side list

### Fixed

- *(ui)* make the title bar drag zone work beneath menus and buttons

### Other

- *(ui)* move the tab strip and central area into their own modules
- *(ui)* split the app into topic modules
- reorganize source tree into domain modules

## [0.1.3](https://github.com/pdf3rn/omni-term/compare/v0.1.2...v0.1.3) - 2026-10-22

### Fixed

- *(ui)* keep egui visuals in sync with the app theme

## [0.1.2](https://github.com/pdf3rn/omni-term/compare/v0.1.1...v0.1.2) - 2026-10-22

### Added

- *(secrets)* switch between keyring and master key with optional migration
- *(ui)* replace checkboxes with toggle switches
- *(ui)* add an integrated title bar with window controls and menu

### Other

- *(packaging)* add app icons and a macOS app bundle

## [0.1.1](https://github.com/pdf3rn/omni-term/compare/v0.1.0...v0.1.1) - 2026-10-22

### Added

- *(ui)* add inline password reveal and clipboard shortcuts in text fields
- *(settings)* ask whether to restore the previous session at startup
- *(settings)* pick any installed monospace font for the terminal
- *(theme)* add ANSI palette selection and styled rendering
- *(ui)* enrich no-tabs screen with actions, recents and shortcuts
- *(settings)* move settings into a tabbed page with cards
- *(sidebar)* enrich connection rows with type icon, env pill and relative time

### Fixed

- *(sidebar)* open new connection directly from the plus button
- *(tabs)* prevent renaming the settings tab
- *(sidebar)* truncate connection name and detail to fit their space

### Other

- *(release)* enable git-only releases in release-plz
- *(ui)* extract reusable components into a ui_kit module
- *(ui)* unify button styles with solid and outline helpers

## [0.1.0]

Primera versión.

### Añadido
- Emulador de terminal nativo con pestañas, paneles divididos, búsqueda, zoom, selección por palabra y línea, y menú contextual.
- Conexiones guardadas en SQLite: SSH (claves, agente, salto, túneles), Telnet y puerto serie.
- Conexiones de archivos SFTP, FTP/FTPS y SMB con explorador de dos paneles y transferencias.
- Secretos en el llavero del sistema, con clave maestra como respaldo.
- Hosts conocidos, snippets con variables, paleta de comandos, menú superior, temas y acentos.
- Restauración de sesión, de tamaño y posición de la ventana.
