# Changelog

Todos los cambios notables de omni-term. A partir de la 0.1.0 este archivo lo actualiza release-plz a partir de los commits convencionales.
El formato sigue [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/) y el proyecto usa [versionado semántico](https://semver.org/lang/es/).

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
