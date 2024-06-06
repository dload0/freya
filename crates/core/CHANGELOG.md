# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/dload0/freya/compare/freya-core-v0.2.1...freya-core-v0.3.0) - 2024-06-06

### Added
- Revamp internal text selection ([#647](https://github.com/dload0/freya/pull/647))
- Reactive Window data ([#637](https://github.com/dload0/freya/pull/637))
- Reactive Platform data ([#635](https://github.com/dload0/freya/pull/635))
- `use_preferred_theme` ([#631](https://github.com/dload0/freya/pull/631))

### Fixed
- Proper accessibility reactivity ([#648](https://github.com/dload0/freya/pull/648))
- Fix performance dropping rapidly after selecting a text for some time ([#624](https://github.com/dload0/freya/pull/624))
- Out of sync element ids on events ([#609](https://github.com/dload0/freya/pull/609))

### Other
- process all queued keyboard events at once ([#629](https://github.com/dload0/freya/pull/629))
- release-plz.toml
- Only release crates under /crates
