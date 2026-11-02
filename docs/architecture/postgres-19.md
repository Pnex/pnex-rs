# PostgreSQL 19 : nouveautés et plan de migration (graphe, histoire des liens)

Statut : **plan, rien d'implémenté** (2026-10-31). Prolonge l'annexe A3 de
`ontology.md` (« PostgreSQL 18 maintenu, passage à 19 quand il sera en
GA ») et l'entrée roadmap du 2026-10-10 (retour à 18 après 19beta4).

## 1. Où en est PG 19 (vérifié 2026-10-31)

- **RC1 publiée le 2026-10-15**, GA pas encore datée (notes de version
  officielles « as of 2026-10-09 »).
- **SQL/PGQ (`CREATE PROPERTY GRAPH`, `GRAPH_TABLE`) retiré** de PG 19
  le 2026-09-07, avant la GA. Retour possible au plus tôt en **PG 20
  (sept. 2027)**. Ce qu'on avait constaté sur 19beta4 reste vrai.
- **`UPDATE/DELETE … FOR PORTION OF` absent** : ni dans les notes ni dans
  le synopsis officiel d'`UPDATE` en 19, alors que des guides tiers
  l'annoncent. Ne pas planifier dessus.
- Pas de nouveauté sur les CTE récursives (`SEARCH` / `CYCLE` existent
  depuis PG 14 et suffisent).

## 2. Ce que PG 19 apporte à pnex

| Nouveauté PG 19 | Usage pnex | Gain |
|---|---|---|
| `range_minus_multi()` / `multirange_minus_multi()` | Histoire d'un lien ou d'un acteur : **périodes sans lien** (trous entre deux mandats, entre deux placements) | Calcul des trous en SQL au lieu de Rust ; utile à la vue Histoire des dossiers |
| `INSERT … ON CONFLICT DO SELECT … RETURNING` | Get-or-create : identité `objects (org_id, type_key, native_id)` (aujourd'hui `DO UPDATE` factice), résolution/import d'ontologie, imports open data | Une requête au lieu d'un `DO UPDATE` qui réécrit la ligne (pas de tuple mort, pas de trigger déclenché pour rien) |
| Fenêtres `IGNORE NULLS` (`lag`, `last_value`…) | Dernière valeur connue d'une propriété temporelle, frises de dossier | Remplace les sous-requêtes « dernière ligne non nulle » |
| `REPACK (CONCURRENTLY)` | `resource_edges`, `objects` : l'historique ne supprime jamais (D179), la table grossit, les index partiels se fragmentent | Réorganisation en ligne, sans verrou exclusif long |
| Planificateur : `NOT IN`/`LEFT JOIN` → anti-jointure, agrégation avant jointure | Requêtes « objets sans lien », compteurs du graphe | Gratuit, sans changement de code |
| `COPY TO … (FORMAT json)` | Exports (dossiers, ontologie) | Optionnel ; nos exports passent par l'API, gain faible |
| Autovacuum parallèle + score de priorité, `max_locks_per_transaction` = 128 | Tables d'ontologie et partitions éventuelles | Exploitation |

**Rien de tout ça n'est un graphe natif.** Le graphe reste « jointures +
CTE + `petgraph` » (A3). PG 19 est une mise à jour d'hygiène, pas un
changement d'architecture.

## 3. Histoire des graphes : ce qui ne dépend pas de PG 19

Le vrai manque est dans notre code, et PG 18 suffit :

1. **Graphe à une date** — `services/ontology/graph.rs` (`reachable_edges`
   et `load`) ne parcourt que les liens ouverts (`valid_to IS NULL`), alors
   que `links.rs` sait déjà filtrer à une date (`valid_at(as_of)`).
   → ajouter `as_of` au parcours (même prédicat que `links.rs`), au
   service puis à la route graphe (OpenAPI, IAM, exposition MCP dans le
   même commit). Les objets sont filtrés sur leur fermeture, comme dans
   `query.rs`.
2. **Défilement dans le temps** — une route qui renvoie les instants de
   changement du sous-graphe (union des `valid_from`/`valid_to` des liens
   chargés) ; l'UI rejoue l'état à chaque instant sans requête par pas.
   Le lecteur temporel partagé (`components/transport.rs`, D220) pilote
   la date.
3. **Diff entre deux dates** — liens valides en A et pas en B (et
   inversement) : deux appels `as_of` comparés côté `pnex-core`, pur Rust
   testable sans base.
4. **Cohérence de l'histoire** — aujourd'hui seul le lien **ouvert** est
   unique par paire (`uniq_resource_edges_open_pair`) ; deux liens clos
   peuvent se chevaucher. PG 18 sait l'interdire :
   colonne `validity tstzrange GENERATED ALWAYS AS (tstzrange(valid_from,
   valid_to)) STORED` + `UNIQUE (org_id, relation, source_kind, source_id,
   target_kind, target_id, validity WITHOUT OVERLAPS)` (extension
   `btree_gist`, contrib, déjà dans l'image alpine). **À trancher** : un
   chevauchement est-il parfois légitime (deux sources qui datent
   différemment le même mandat) ? Si oui, garder la règle actuelle et
   signaler le chevauchement à l'analyste au lieu de le refuser.

Ces quatre points se font **avant** la migration, sur PG 18 ; seul le
calcul des trous (§2, `range_minus_multi`) attend PG 19.

## 4. Plan de migration

| Étape | Contenu | Condition |
|---|---|---|
| **M0** | Job CI non bloquant `postgres:19rc1-alpine` (base de référence + tests backend ciblés), à côté du job 18 | Maintenant |
| **M1** | Vérifs de rupture : pas d'index `btree_gist` sur `inet`/`cidr` (aucun aujourd'hui), pas de CR/LF dans les noms de base/rôle, pas d'auth MD5 (warnings en 19), dumps toujours faits avec `standard_conforming_strings = on`, JIT désormais coupé (rien ne s'en sert), TOAST `lz4` par défaut (neutre) | Avec M0 |
| **H1–H3** | §3 points 1 à 3 (graphe à une date, défilement, diff) | Sur PG 18, indépendant |
| **H4** | §3 point 4 après arbitrage | Sur PG 18, indépendant |
| **M2** | `compose.yaml`, `compose.ci.yaml`, `.github/workflows/ci.yml` → `postgres:19-alpine` ; `migrations.md` à jour | **GA de PG 19** |
| **M3** | Chart Helm (`pnex-deploy`) + dev.pnex.io : `pg_upgrade --check` puis `pg_upgrade`, ou dump/restore (beta : pas de compat à préserver) ; données de dev = pas de bascule logique | Après M2 |
| **M4** | Adoption : `ON CONFLICT DO SELECT` (identité d'objet, imports), `range_minus_multi` (trous dans la vue Histoire), `IGNORE NULLS` (dernière valeur), `REPACK CONCURRENTLY` dans la doc d'exploitation | Après M3 |
| **M5** | Veille SQL/PGQ pour PG 20 : il ne serait qu'une syntaxe sur nos tables (A3) ; adopter seulement si les chemins de longueur variable arrivent avec | PG 20 bêta (mi-2027) |

Contraintes qui ne bougent pas : PostGIS et pgvector (`geo-layers.md`,
`doc-search.md`) ne sont pas encore montés ; s'ils le sont avant M2, M2
attend leurs images 19 **arm64**. Aucune fonctionnalité de M4 ne doit
rendre PG 18 inutilisable avant que M3 soit fait partout.

## 5. Sources

- Notes de version officielles PG 19 : https://www.postgresql.org/docs/19/release-19.html
- Retrait de SQL/PGQ : https://neon.com/postgresql/postgresql-19-new-features
- Couverture bêta (avant retrait) : https://www.infoq.com/news/2026/06/postgresql-19-graph-queries/
