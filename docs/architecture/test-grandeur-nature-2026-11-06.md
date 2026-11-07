# Test en grandeur nature du 2026-11-06 : constats et plan de correctifs

Test mené sur dev.pnex.io (image 2d3d38e, org de test dédiée) : dix groupes
industriels français suivis de bout en bout avec des données publiques
uniquement (SIRENE, ratios BCE/INPI, GLEIF, registre des entreprises, presse et
sites des entreprises). Les noms réels restent hors du dépôt (règle « aucune
entreprise réelle »), seule la mécanique est consignée ici.

## 1. Ce qui a été chargé et calculé

| Étape | Volume | Durée | Résultat |
|---|---|---|---|
| Unités légales SIRENE (CSV 4,3 Go, 129 parties) | 30,3 M lignes | 6 min 10 | OK |
| Ratios financiers BCE/INPI (0,9 Go) | 6,8 M lignes | 1 min 48 | OK, BOM géré |
| Géolocalisation INSEE (4,2 Go, séparateur `;`) | 38,0 M lignes | 5 min 40 | OK |
| Établissements SIRENE (10 Go, trois `--append` de 3,4 Go) | 44,5 M lignes | 26 min | OK après reprise (voir C2) |
| GLEIF entités (5,07 Go, 338 colonnes, un seul envoi) | 3,49 M lignes | 17 min | OK |
| GLEIF relations | 0,49 M lignes | < 1 min | OK |
| Pipeline périmètre (30 M lignes × 18 préfixes) | 146 lignes | 8,7 s | OK |
| Pipeline finances (6,8 M × périmètre) | 818 bilans, 2 328 points | 3,7 s | OK |
| Pipeline GLEIF (3,49 M + 0,49 M) | 26 membres, 29 liens | 33 s | OK, contrôle `min_rows` a bien bloqué la 1re version |
| Pipeline sites (44,5 M × 105 SIREN) | — | — | **échec**, worker tué (C3) |

Ontologie : 10 têtes, registre (sièges, 84 établissements, dirigeants), GLEIF,
82 filiales candidates en propositions, 88 liens web sourcés (fournit, possède,
partenaire), 57 dirigeants publics, 56 articles de presse, 10 dossiers
d'enquête. Le pack d'org `supply_chain` (lien `supplies`, `partner_of`, clé
`source_key`) a été installé par YAML pour le test.

## 2. Constats, du plus grave au moins grave

| # | Constat | Gravité | Porteur | État |
|---|---|---|---|---|
| C1 | Worker de pipelines jamais déployé (`pipelineWorker.enabled: false`) : tout run restait `queued` | bloquant | Shan (chart) | déployé (helm rev 17) |
| C2 | Le rôle `pnex_pipeline_worker` n'a pas `USAGE` sur `public` (CNPG retire le droit à PUBLIC) : « relation does not exist » en boucle | bloquant | c2 | GRANT manuel en prod ; migration 000102 non commitée |
| C3 | Worker de pipelines tué (OOMKilled 4 Gi) sur 105 × 44,5 M : `prefer_hash_join=false` planifie un SortMergeJoin qui trie 44,5 M lignes hors du pool | bloquant | c2 | `prefer_hash_join=true` + test, non commité |
| C4 | Un job qui tue le worker est rejoué jusqu'à `MAX_ATTEMPTS` : le worker tombe à chaque reprise (crash en boucle), le run reste `running` entre deux | majeur | c2 | à faire |
| C5 | La conversion des uploads tourne sur tous les pods, flow-worker compris (1 Gi) : OOMKilled, flows déployés coupés | majeur | c2 | conversion réservée aux pods API + vidage par taille réelle, non commité ; flow-worker à 3 Gi en attendant |
| C6 | Une jointure grosse × grosse (44,5 M × 38 M) ne peut pas déborder sur disque : arrêt propre `pipeline-memory-exceeded`, mais aucun moyen de la faire | majeur | c2 | à concevoir (jointure partitionnée ou tri-fusion compté dans le pool) |
| C7 | Après la mort d'un pod, une conversion attend 10 min (battement) avant reprise ; vécu deux fois de suite | mineur | c2 | à arbitrer (seuil plus court ou reprise au démarrage du pod) |
| C8 | Un import depuis une table ne peut pas écrire de séries datées : `observations` exige `per_object` (source API) ; les finances restent en table | majeur (produit) | c2 / ontologie | à faire : `observations` sur source `dataset` |
| C9 | Pas de `when` sur un lien d'import : il faut une table par type de lien (trois imports au lieu d'un) | moyen | ontologie | à faire |
| C10 | `invalid: skip` sur un attribut de lien n'ignore pas une date partielle (`2026-03`) : l'import entier est refusé | moyen | ontologie | à corriger ; contournement SQL |
| C11 | `imports-update` exige `expected_version` sans que la CLI l'aide ; corps d'erreur clair mais friction | mineur | CLI | à faire : `pnex import update` qui relit la version |
| C12 | L'import API « Établissements (registre) » ne prend que la 1re page par société (84 sites pour 10 groupes) | moyen | prospection FR | à vérifier `max_pages` / pagination |
| C13 | GLEIF niveau 2 couvre mal les groupes industriels (filiales majeures sans relation déclarée) | donnée | — | compensé par les candidats « même nom » en propositions |
| C14 | Préfixe de nom seul = bruit (restaurants, SCI) ; filtré par forme juridique et NAF, il reste quelques faux positifs | donnée | — | propositions à valider par un humain |
| C15 | Carte : au retour sur `/map` par le menu, 0 marqueur (vue périmée de l'ancienne carte, puis `setItems([])` au `load`) | majeur (UI) | 3a | corrigé dans `js/map.js`, non commité |
| C16 | Loader 16 px : l'animation tourne mais le motif de tirets (12 px écran, `non-scaling-stroke`) est plus long qu'un faisceau : paraît figé | mineur (UI) | design | à trancher : motif proportionnel ou variante < 24 px |
| C17 | Dossier : une personne ne s'attache qu'après déclaration de la base légale (`case-legal-basis-required`) | voulu | — | base « test interne, données publiques » déclarée |
| C18 | Vue Argent vide : elle ne lit que des événements avec montant + payeur/bénéficiaire ; les liens `owns`/`supplies` portant un montant ne l'alimentent pas | majeur (produit) | dossiers | à concevoir (voir §3) |
| C19 | Données sources : comptes consolidés en double (2025 = 2024), résultat net à 0 sur des bilans K | donnée | — | garder la source, signaler |
| C20 | Org qualiwatch : import programmé « partners » en `import-run-stale` deux nuits de suite | majeur | c2 / ingestion | à investiguer |
| C21 | Worker de pipelines : erreurs de connexion S3 (`transport error of kind Connect`, essais 7–9 sur 10) | à surveiller | c2 | NetworkPolicy ou démarrage RustFS |

## 3. Plan de correctifs

### Lot A — remettre la chaîne de données d'aplomb (c2, avant tout autre test)

1. Commiter et déployer C2 (migration 000102), C3 (`prefer_hash_join`), C5
   (conversion hors flow-worker, vidage par taille réelle).
2. C4 : un job qui a tué son worker une fois passe en quarantaine
   (`pipeline-worker-lost` dès la 2e mort, pas de reprise en boucle) ; le run
   ne reste jamais `running` sans battement.
3. C6 : jointure grosse × grosse. Proposition : jointure par hachage
   partitionnée sur la clé (les deux côtés écrits en N fichiers de spill),
   ou tri-fusion dont le tri est compté dans le pool. Mesure sur 44,5 M × 38 M.
4. Remettre le flow-worker à 1 Gi dans `values-dev.yaml` une fois C5 en prod.
   Audit du 2026-11-06 (`audit-2026-11-06.md` A7) : `sweep_dead` remettait en
   file à 3 min alors que `begin()` ne libère la transaction ouverte qu'à
   10 min (`dataset-transaction-open`) — corrigé avec A8 et B2 en 8c8f00d0.
5. C20 et C21 : cause de `import-run-stale` sur qualiwatch ; erreurs S3 du
   worker.

   Piste C20 (audit B1) : le diff d'import est calculé hors du verrou d'org ;
   fc_certifiers, fc_sheets, fc_partners et SIRENE écrivent les mêmes
   `organization` la même nuit, le second échoue en `import-run-stale`.

### Lot B — imports et ontologie (structurel)

1. C8 : `observations` sur une source `dataset` (colonnes `@queried`, `metric`,
   `unit`, `start`, `end`, `value`), sans `per_object`. Les comptes publiés
   deviennent des séries sur la fiche.
2. C9 : `when` sur un lien d'import, même forme que sur un objet.
3. C10 : `invalid: skip` respecté sur un attribut de lien.
4. C11 : `pnex import update <id> --mapping @file` qui relit la version.
5. C12 : pagination de l'import « Établissements (registre) ».

### Lot C — dossiers et argent

1. C18 : la vue Argent lit aussi les liens qui portent un montant (`owns` avec
   prix d'acquisition, contrat `supplies` avec valeur) comme des paiements
   datés, avec leur source en pièce. Sinon chaque paiement reste un événement
   saisi à la main.
2. Rattacher automatiquement à un dossier de compte ce que le graphe ajoute
   ensuite (nouveau dirigeant, nouvel article), selon les règles de veille
   existantes.

### Lot D — interface (3a, design)

1. C15 : commiter le correctif de `js/map.js`, `task js:build`, redéployer le
   front, rejouer le retour sur `/map`.
2. C16 : décider du loader sous 24 px.

### Lot F — des liens qui reflètent la réalité (retour de Shan)

Constat de Shan : pour un grand groupe de défense, une vingtaine de liens de
presse et un site ne ressemblent pas à la réalité (la filière emploie plus de
200 000 personnes en France). Aucun registre ne publie la sous-traitance entre
entreprises privées ; les sources publiques qui s'en approchent, en masse :

1. **DECP** (données essentielles de la commande publique, data.gouv.fr) :
   acheteur, titulaire (SIRET), montant, date de chaque marché public, ministère
   des Armées compris. Donne des liens réels **et** des paiements datés pour la
   vue Argent (C18). Gros fichier : pipeline (filtre sur les SIREN suivis et
   leurs filiales), puis import liens `supplies` + événements de paiement.
2. **TED** (attributions européennes) : cotraitants et sous-traitants déclarés.
3. **Annuaires des syndicats professionnels** (défense terrestre, aéronautique,
   naval) : l'écosystème par filière, sans lien bilatéral.
4. **Brevets co-déposés** (INPI) : partenariats techniques.

Au niveau mondial (l'écart est plus grand encore : des centaines d'entités
consolidées par groupe, des dizaines de milliers de fournisseurs) :

5. **Périmètre de consolidation des documents de référence annuels** (note des
   comptes consolidés) : toutes les sociétés du groupe, pays et pourcentage.
   Source la plus complète pour les filiales mondiales des groupes cotés ; PDF à
   extraire.
6. **SEC EDGAR, Exhibit 21 du 10-K** : liste officielle des filiales des
   sociétés cotées aux États-Unis (le pack marchés lit déjà EDGAR).
7. **USAspending.gov** : contrats fédéraux américains, défense comprise, avec
   les **sous-contrats déclarés** (qui sous-traite à qui, montant, date). Seule
   source publique massive de sous-traitance réelle ; bulk de plusieurs dizaines
   de Go, cas d'usage type des pipelines (et de C6).
8. **Marchés publics étrangers** : TED (UE), Find a Tender (Royaume-Uni),
   registres canadien et australien.

À dire dans l'UI : un lien porte sa source, l'absence de lien ne prouve rien ;
la sous-traitance privée de rang 2 et 3 reste partielle.

### Lot E — reprendre le test (après A et D en prod)

1. Relancer le pipeline des sites en trois étapes (filtre, puis géolocalisation),
   importer les établissements, les rattacher aux dossiers.
2. Monter les comptes publiés en séries (après B1).
3. Saisir les paiements documentés (contrats, cessions, augmentations de
   capital) et vérifier la vue Argent de chaque dossier.
4. Relire les données web (sources secondaires ou anciennes, actualités « À
   valider ») et les 86 filiales proposées.

## 4. Reprise du 2026-11-07 (dev en 0.2.0, 9d86cbc)

- **C3 vérifié** : pipeline « Check: big joins (C3) », vraie jointure
  `sirene_establishments` (44,5 M) × `followed_sirens` (105), puis
  `sirene_geolocation` (38 M) × sites : 14 s, aucune mort du worker, 766 sites
  (identique à la liste en dur de « Group sites » v3), 762 positions.
- **C8 vérifié** : import « Published accounts (BCE/INPI) » depuis
  `group_financial_series` (`observations: { type: organization, property: siren }`) :
  2328 lignes appliquées, séries visibles sur la fiche (59 valeurs pour un
  groupe, social et consolidé) ; une 2e exécution n'écrit rien.
  Reste : la source ne donne qu'une date de clôture, la période vaut un jour
  (`start` = `end`) ; à dériver dans le pipeline (exercice de 12 mois).
- Restent du lot E : vue Argent dossier par dossier (E3), relecture humaine des
  données web et des 86 filiales proposées (E4).
- **Adresse des sites** (retour de Shan) : aucun site n'avait d'adresse, le type
  `map_pin` n'en portait pas. Pack `companies` 2.2.0 : propriété `address` ;
  imports du registre (`companies_fr` 4.3.0, `prospecting_b2b_fr` 3.6.0,
  `quali_watch` 3.5.0) : `adresse` du registre. « Group sites » v4 : vraie
  jointure (plus de liste en dur) et adresse SIRENE, 766 sites en 14 s.
  Mapping de l'import des sites à compléter (`address: address`) une fois
  l'image déployée et les packs montés de version sur dev.
  Piste pour plus tard : adresse déduite des coordonnées (géocodage inverse)
  pour les sites qui ont une position mais pas d'adresse.
