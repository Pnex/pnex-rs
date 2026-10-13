# Moteur : récurrence, export .ics, CalDAV, contraintes — fiche d'analyse

> Étude du 2026-10-13 : crates.io, GitHub, lecture des dépôts permissifs.
> Rien n'a été compilé. RustSec : aucune alerte connue pour `rrule` ni
> pour `icalendar`.

## `rrule` (fmeringdal/rust-rrule) — candidat récurrence

- **Licence** : MIT OR Apache-2.0 (fichiers `LICENSE-*` lus).
- **Version** : 0.14.0 du 2025-04-20, sans commit depuis. Le dépôt n'est
  pas archivé, mais des issues de 2026 restent sans réponse :
  - doublon quand un RDATE égale une occurrence RRULE ;
  - DATE non accepté dans EXDATE ;
  - `Display` qui perd l'ordinal 1 de BYDAY (le texte de la règle ne
    fait plus l'aller-retour).
  
  **Maintenance au ralenti** : c'est un risque à suivre, pas un motif
  d'exclusion.
- **Dépendances** : `chrono` (features par défaut, dont `wasmbind`),
  `chrono-tz` (obligatoire), `regex`, `log`, `thiserror`.
  `pnex-core` n'a aujourd'hui ni chrono-tz ni regex : la table des fuseaux
  et regex alourdissent le bundle wasm.
- **wasm32** : **non prouvé**. Pas de CI wasm, l'issue #53 est ouverte
  depuis 2022. Comme tout est du Rust pur, la compilation est plausible.
  `Local` est à proscrire : seuls des fuseaux IANA explicites.
- **Couverture RFC 5545** :
  - FREQ de la seconde à l'année, INTERVAL, COUNT, UNTIL, WKST ;
  - tous les BYxxx, dont BYSETPOS et BYDAY avec ordinal ;
  - ensemble de règles : DTSTART avec TZID, plusieurs RRULE, RDATE,
    EXDATE.
  
  Le validateur RFC est toujours actif et 39 tests RFC sont présents.
- **Garde-fous** : années bornées, INTERVAL maximal par fréquence,
  100 000 itérations sans résultat au plus.
  **Piège** : ces bornes ne s'appliquent qu'à l'énumération limitée. Il
  faut toujours une énumération bornée par un nombre, jamais la forme non
  vérifiée sur une règle venue de l'utilisateur.
- **Fuseaux et changement d'heure** : l'heure murale est conservée au
  changement d'heure (testé).
- **API** : bornes après/avant combinables puis énumération limitée ;
  parsing et sérialisation du texte RRULE.

## `icalendar` (hoodie/icalendar) — candidat export .ics

- **Licence** : MIT/Apache-2.0 (lue). Version 0.17.14 (2026-09-29),
  releases mensuelles.
- **Export** : événements et tâches ; RRULE, RDATE, EXDATE,
  RECURRENCE-ID.
- **Limites** :
  - **pas de VTIMEZONE généré**, seulement `X-WR-TIMEZONE`. Les TZID
    IANA sont acceptés par la plupart des clients, mais ce n'est pas
    strictement conforme ;
  - sous wasm, elle active `uuid` v4, contraire à la règle de
    `pnex-core`.
  
  **Côté backend uniquement**, avec le parser désactivé pour un export
  pur.

## RustiCal — CalDAV (étude seulement)

- **Licence : AGPL-3.0** (lue). On ne reprend ni code ni crate.
- **Couverture** : CalDAV et CardDAV, création de calendrier, requêtes et
  multiget, sync-collection, WebDAV Push, OIDC ; clients DAVx5, Apple,
  Thunderbird, GNOME. Ni free-busy ni planification RFC 6638 trouvés
  (recherche par grep, non vérifiée à fond).
- **Faisabilité PNEX** :
  - même en lecture seule, il faut une couche WebDAV complète : XML
    multistatus, découverte `.well-known`, requêtes, ETags, sync ;
  - il faut aussi une authentification Basic ou par mot de passe
    d'application, que l'OIDC Rauthy ne fournit pas ;
  - l'écriture obligerait à réconcilier un iCal arbitraire avec notre
    modèle, ce qui ouvre une nouvelle surface d'attaque (XML non fiable,
    R1/R2).
  
  **Hors V1.** Un **flux d'abonnement `.ics` en lecture seule**, à jeton
  révocable par utilisateur × org, couvre l'essentiel.

## Timefold — quickstart « maintenance scheduling » (Apache-2.0)

Modèle (concepts) :

- **Fenêtre de travail** de jours ouvrés. Les week-ends sont exclus ; les
  jours fériés ne sont pas gérés.
- **Granularité : le jour.** Une intervention dure un nombre entier de
  jours ouvrés.
- **Équipe** : sans compétences dans le quickstart.
- **Intervention** : durée, plus trois dates (**prête au plus tôt**,
  **échéance**, **idéale**), plus des **étiquettes** (une zone, par
  exemple) qui interdisent le chevauchement.
- Le solveur choisit l'équipe et la date de début de chaque intervention.
  Tout est affecté.

Catalogue de contraintes :

| Contrainte | Type | Pénalise |
|---|---|---|
| Conflit d'équipe | dure | une équipe sur deux interventions qui se chevauchent (jours de chevauchement) |
| Début minimal | dure | un début avant « prête » (jours d'avance) |
| Échéance | dure | une fin après l'échéance (jours de retard) |
| Avant l'idéale | souple ×1 | une fin trop tôt, car une maintenance précoce sera refaite plus tôt |
| Après l'idéale | souple ×1 000 000 | une fin après l'idéale, car l'échéance est proche |
| Conflit d'étiquettes | souple ×1 000 | un chevauchement d'interventions qui partagent des étiquettes |

Les poids sont surchargeables et chaque pénalité est expliquée. On
retient le **vocabulaire prête / idéale / échéance** et le catalogue,
pas le solveur.

## Recommandation

- **Récurrence** : `rrule` 0.14, version épinglée, avec ces règles :
  - toujours une énumération bornée ;
  - fuseaux IANA uniquement ;
  - stockage du texte RRULE canonique, de DTSTART et du TZID ;
  - revalidation côté backend.
  
  **Avant de s'engager** : `cargo check --target wasm32` (via
  `scripts/guarded.sh`) et mesure du bundle. Si c'est trop lourd,
  l'aperçu des dates vient du backend et `pnex-core` ne garde que les
  DTO. Écrire des tests sur nos cas pour couvrir le doublon RDATE et le
  `Display`.
- **Export** : `icalendar` 0.17 côté backend, sans parser, avec le TZID
  IANA seul.
- **CalDAV** : hors V1.
- **Optimisation** : plus tard, un ordonnanceur heuristique maison qui
  reprend le catalogue ci-dessus.

## Sources

- https://crates.io/crates/rrule · https://github.com/fmeringdal/rust-rrule (README, validator, tests, issue #53)
- https://crates.io/crates/icalendar · https://github.com/hoodie/icalendar (Cargo.toml, README, CHANGELOG)
- https://github.com/lennart-k/rustical (LICENSE, README)
- https://github.com/TimefoldAI/timefold-quickstarts/tree/stable/use-cases/maintenance-scheduling
