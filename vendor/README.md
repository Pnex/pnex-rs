# vendor/ — dépendances vendored

## edgelinkd

- **Source** : notre fork <https://github.com/Pnex/edgelinkd>, synchronisé depuis l'amont
  <https://github.com/oldrev/edgelinkd> (moteur de flow compatible Node-RED en Rust).
- **Commit épinglé** : `80cc1e0` (2026-11-07) : amont v4.1.16 fusionné, plus les mises à
  jour de dépendances. L'amont ne publie pas de release versionnée, donc on épingle un SHA,
  jamais un tag.
- **Licence** : Apache-2.0 (`vendor/edgelinkd/LICENSE`), Copyright Li Wei and other
  contributors. Le code du fork reste sous sa licence d'origine (workspace : MIT).
- **Patchs du cœur autorisés dans le fork** (2026-11-07, plus « ne jamais patcher ») :
  - SEC-3 : l'environnement du processus n'est pas copié dans le magasin d'env du moteur ;
  - SEC-67 : budget du nœud `function` (128 Mio de mémoire, 1 Mio de pile, 10 s par appel),
    qui partage le gestionnaire d'interruption du `timeout` par nœud de l'amont ;
  - `dirs-next` retiré.

  On étend toujours d'abord par nœuds custom (`crates/pnex-node-*`) et par notre binaire
  (`crates/pnex-flow-runtime`). Un patch du cœur se fait dans le fork, avec sa raison, et
  part en PR chez oldrev quand il est utile à l'amont.
- **Mise à jour** : synchroniser le fork depuis oldrev, puis bumper le submodule (SHA
  ré-épinglé ici) et faire passer les tests de `pnex-flow-runtime` et des `pnex-node-*`.
  Depuis 80cc1e0, le nœud debug sort du JSON compact (`"k":v`) : un test ne doit jamais
  chercher `"k": v` avec une espace.
- **Sous-module `3rd-party/node-red` volontairement NON initialisé** (clone Node-RED complet,
  ~100 Mo, inutile à la compilation de `edgelink-core`). Ne jamais faire
  `git submodule update --recursive` ici.
- **Intégration** : `edgelink-core` est consommé en path-dependency
  (`default-features = false, features = ["core"]`) depuis les `crates/pnex-node-*` et
  `crates/pnex-flow-runtime`. Le backend Loco **ne lie jamais** ces crates (isolation
  process, mode B).

## CoolProp

- **Amont** : <https://github.com/CoolProp/CoolProp> — bibliothèque C++ de propriétés
  thermophysiques (fluides purs, mélanges, air humide). Épinglé au **tag `v8.0.0`**
  (SHA `ae81610e7d23efc57f9d051c8e70a4d66e87537f`) ; contrairement à edgelinkd, un tag
  de release versionné existe en amont → on épingle le tag (le SHA est juste documenté).
- **Licence** : MIT (`vendor/CoolProp/LICENSE`), compatible avec la licence MIT du
  workspace.
- **Patch autorisé ici** (contrairement à edgelinkd) :
  `vendor/patches/coolprop/exception-guards.patch` — les wrappers KSI dépréciés d'amont
  (`Props1`, `PropsS`, `cair_sat`) ne rattrapent pas leurs exceptions C++ ; une exception
  qui traverse la frontière `extern "C"` est UB (abort) pour l'appelant FFI. Le patch
  ajoute les mêmes try/catch que les autres exports. **Appliqué par le `build.rs` de
  `crates/pnex-coolprop-sys`** (idempotent : `git apply --check`), jamais commité dans
  le submodule. Retour amont possible via PR CoolProp. Effet de bord assumé : après un
  build, `git status` affiche `modified: vendor/CoolProp` — cosmétique et auto-réparant
  (`git -C vendor/CoolProp checkout -- src/CoolPropLib.cpp` ou laisser le `build.rs`
  re-appliquer au build suivant). Pour taire le bruit localement :
  `git config submodule."vendor/CoolProp".ignore dirty` (config locale, à refaire après
  un clone frais).
- **Intégration** : compilée en bibliothèque **statique** par CMake au build du crate
  `crates/pnex-coolprop-sys` (FFI brut, 71 exports de `CoolPropLib.h`), consommée par le
  wrapper safe `crates/pnex-coolprop` (appel **in-process** — remplace le plan initial
  d'un service HTTP séparé, cf. coolprop-rs). Le backend Loco peut lier `pnex-coolprop`
  directement ; l'état CoolProp (config, errstring, handles AbstractState) est
  process-global et sérialisé par le wrapper.
- **Si le submodule n'est pas initialisé** (checkout frais), le `build.rs` clone
  automatiquement le tag épinglé en repli (`git submodule update --init vendor/CoolProp`
  reste la voie propre).
