# vendor/ — dépendances vendored

## edgelinkd

- **Amont** : <https://github.com/oldrev/edgelinkd> — moteur de flow compatible Node-RED en Rust.
- **Commit épinglé** : `d0a5e114468ee1b26147de55cdca10484ade6b05` (master, 2026-01-19). Pas de
  release versionnée en amont (nights Windows uniquement) → on épingle un SHA, jamais un tag.
- **Licence** : Apache-2.0 (`vendor/edgelinkd/LICENSE`), Copyright Li Wei and other contributors.
  Compatible avec l'AGPL-3.0-or-later du workspace : code vendored **séparé et non modifié**.
- **Règle absolue : ne jamais patcher.** Toute modification du cœur EdgeLinkd est interdite
  (décision PRD §3) — on étend par nœuds custom (`crates/pnex-node-*`) et par notre binaire
  (`crates/pnex-flow-runtime`). Les retours amont passent par issue/PR chez oldrev ; les mises à
  jour se font par bump du submodule (SHA re-épinglé + note dans `docs/architecture/flow-engine.md`).
- **Sous-module `3rd-party/node-red` volontairement NON initialisé** (clone Node-RED complet,
  ~100 Mo, inutile à la compilation de `edgelink-core`). Ne jamais faire `git submodule update --recursive` ici.
- **Intégration** : `edgelink-core` est consommé en path-dependency
  (`default-features = false, features = ["core"]`) depuis `crates/pnex-node-sql` et
  `crates/pnex-flow-runtime`. Le backend Loco **ne lie jamais** ces crates (isolation process, mode B).
