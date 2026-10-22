# Propositions et décisions (D367, D371–D377)

| | |
|---|---|
| Statut | **P0, P1, P2 et P4 livrés le 2026-10-22** ; P3 à faire |
| Décision mère | D367 (`ontology.md` §11) : la proposition est un objet du noyau, priorité 1 |
| Repose sur | ontologie (D176–D191), propositions des dossiers (`cases-workspace.md` K30, K33), attributions vocales (`media-review.md` D275), packs (D190, D368) |
| Numérotation | **D371…D377**, lots **P0…P4** |

## 1. Constat

Quatre mécanismes disent aujourd'hui « une machine pense que… » :

| Où | Forme | Défaut |
|---|---|---|
| Dossiers | table `case_proposals` (détecteurs, empreinte, retenue / rejetée) | propre au dossier, ne crée que des événements |
| Voix | attributions `origin = voice`, `status = proposed` | propre aux médias |
| Labo vision | suggestions dans les images d'un projet | propre au labo |
| Packs | état dans un champ : `filiale_de.etat = a_valider`, `a_pour_site.statut = propose`, `signal_commercial.etat`, constats `quali_watch` | la proposition est déjà une donnée vivante ; chaque requête doit filtrer ; pas de boîte commune ; pas de journal des décisions |

## 2. Ce qui est une proposition, ce qui ne l'est pas

- **Proposition** : une écriture que la plateforme n'a pas le droit de faire seule parce qu'elle n'est pas sûre que ce soit **vrai**. Exemples : un lien de groupe tiré de Wikidata, le site web d'une organisation, une société retrouvée par son nom, la voix d'une personne, un événement détecté, une extraction par LLM.
- **Pas une proposition** : un **statut de tri** d'une donnée vraie. Un signal BODACC existe, c'est un fait ; « retenu » ou « écarté » dit seulement s'il intéresse le commercial. Un constat d'écart produit par une règle déterministe est un fait daté ; son suivi (relances, clôture) est un workflow. Ces champs restent des propriétés métier du pack (voir Q-P1).

## 3. Décisions

### D371 — Une table `proposals` au noyau

Champs :
- `org_id` ;
- `kind` : `object_create`, `object_update`, `link_create`, `link_update`, `link_close` (liste fermée ; les dossiers ajouteront `case_event` en P3) ;
- `target` : le type ou l'objet visé, le type de lien et ses deux extrémités ;
- `diff` : propriétés ou attributs proposés, titre ;
- `base_version` : version de l'objet lue, pour une mise à jour ;
- `evidence` : extraits cités, adresses, identifiants de preuve (D310) ;
- `refs` : objets concernés, pour filtrer la boîte par objet ;
- `origin` : `import:<id>@<run>`, `flow:<id>@<version>:<node>`, `detector:<clé>`, `llm:<modèle>`, `assistant` ;
- `confidence` (0–100, facultatif) ;
- `fingerprint` ;
- `status` : `proposed`, `accepted`, `rejected`, `superseded` ;
- décision : qui, quand, motif ;
- `applied` : l'objet ou le lien écrit ;
- dates.

### D372 — Une empreinte, jamais reproposé

L'empreinte est calculée par le producteur (type, extrémités, clé, valeurs proposées), unique par org. Une proposition rejetée bloque la même empreinte à jamais. Une nouvelle valeur (une autre mère, un autre site) fait une autre empreinte. Une proposition encore ouverte qui reçoit une version plus récente de la même cible passe en `superseded`.

### D373 — Accepter = écrire par le service partagé

Accepter applique la différence par les mêmes services que l'UI (objets, liens) : mêmes validations, mêmes cardinalités, mêmes 409. La décision écrit sous l'utilisateur qui décide, avec la provenance `proposal:<id>` (D184) ; l'origine machine reste lisible sur la proposition. Une mise à jour dont l'objet a bougé depuis `base_version` est refusée (`proposal-stale`) : on la rejette ou on relance le producteur. Accepter et rejeter sont idempotents.

### D374 — Une boîte « À valider », une permission

- **Page** « À valider » : filtres par origine, type et objet ; décision une par une ou en lot. **Pastille** sur la page d'un objet concerné.
- **Permissions** : `proposals:read` (lecteur) et `proposals:decide` (membre). Décider vérifie en plus le droit d'écrire le type visé (D188).
- **Assistant et MCP** : lecture exposée ; décision exclue (« sign-off » : un humain décide).

### D375 — Les producteurs disent « proposer » au lieu d'« écrire »

- **Import** : une entrée d'objet ou de lien d'un mapping peut porter `propose: true`. Le diff produit alors des propositions au lieu d'écritures. Exemple : les liens Wikidata.
- **Nœuds `object_upsert` et `link_upsert`** : option `propose` avec `confidence`. Exemples : le flow de présence web pour les scores moyens, les extractions par LLM.
- Le pack ne déclare plus d'état `a_valider` ; il déclare **qui propose**.

### D376 — Migration des packs existants

- Les liens `filiale_de` de nature `wikidata` encore `a_valider` deviennent des propositions `link_create`, et le lien vivant est fermé. Les liens `a_pour_site` au statut `propose` suivent le même chemin.
- Les attributs `etat` de `filiale_de` et `statut = propose` d'`a_pour_site` disparaissent des packs, avec une nouvelle version des packs ; `statut` ne garde que `confirme` / `rejete`, ou disparaît (voir Q-P1).
- La section « Sites à valider » devient un filtre de la boîte.

### D377 — Acceptation automatique : une règle d'org, tracée

Au-dessus d'un seuil déclaré par le pack et surchargeable par l'org (D368), une proposition est acceptée sans humain. Exemple : un site confirmé par SIREN et NDA.

Elle reste écrite comme proposition, `accepted` par `system` avec la règle citée : le journal des décisions est complet. Une source déclarée fiable (registre officiel) n'est jamais proposée : elle écrit directement, comme aujourd'hui.

**Livré (P4)** : un nœud `link_upsert` `propose: true` reçoit `msg.payload.accept`, le texte de la règle (exemple : `presence_web.seuil_confirme >= 80 (score 92)`). Le service propose puis accepte aussitôt : `decided_by` vide, motif = la règle ; l'UI affiche « Automatique : <règle> ». La même valeur encore ouverte est acceptée ; une valeur déjà rejetée le reste (D372) ; une écriture refusée (cardinalité, validation) laisse la proposition ouverte pour un humain. Le seuil vit dans la fonction du pack, qui le lit par `pack_setting` (D368) : c'est elle qui pose `accept`. Le message vient du code du flow, qui pourrait aussi bien écrire sans proposer : `accept` n'ouvre aucun droit nouveau. Pas fait : `object_upsert` et les imports n'ont pas encore d'acceptation automatique.

## 4. Lots

| Lot | Contenu | Prérequis |
|---|---|---|
| **P0** | Table, service (créer, lister, accepter, rejeter, remplacer), routes, IAM, MCP, tests | — |
| **P1** | Page « À valider », pastille sur l'objet, fiche de l'assistant | P0 |
| **P2** | Producteurs : `propose` dans les mappings et les nœuds ; bascule de Wikidata et de la présence web ; migration D376 | P0 |
| **P3** | `case_proposals` et attributions vocales dans la même table (`kind` dédiés), une seule boîte | P1 |
| **P4** | Acceptation automatique par règle d'org (D377) | D368 |

## 5. Questions (défaut retenu en attendant Shan)

- **Q-P1** : statuts de tri (signal retenu ou écarté, constats) : propriétés métier, pas des propositions. **Défaut : oui.** On ne migre que ce qui dit « est-ce vrai ? ».
- **Q-P2** : données communautaires (Wikidata) en propositions, plutôt qu'en liens vivants à fermer. **Défaut : oui.** Le graphe vivant reste propre.
- **Q-P3** : dossiers et voix unifiés dans la même table (P3) ou fédérés en lecture. **Défaut : même table.**
- **Q-P4** : une proposition sans réponse expire-t-elle ? **Défaut : non** ; filtre « ancienneté » dans la boîte.
- **Q-P5** : une décision humaine peut-elle modifier la valeur proposée avant d'accepter (« accepter avec correction ») ? **Défaut : oui pour les propriétés**, la correction est tracée dans la décision.

## 6. Sécurité

- R1 : org tirée du principal ou du nœud déployé.
- R2 : `proposals:read` / `proposals:decide` + droit d'écriture du type, avec un test 403.
- R4 : `decided_by` est rendu par le nom affiché, jamais par l'e-mail.
- Une preuve citée (texte d'un site inconnu) est une donnée, jamais rendue en HTML (R11).
- L'assistant ne décide pas.

## 7. État

- **P0** (`8776c7ff`) :
  - table `proposals` (migration 000046), cœur `ontology::proposal`
    (types, `slot`, texte d'empreinte haché en SHA-256 par le serveur) ;
  - service `ontology::proposals` (`propose`, `list`, `get`, `decide`) ;
  - routes `GET /api/v1/proposals`, `GET /api/v1/proposals/{id}`,
    `POST /api/v1/proposals/{id}/decision` ;
  - permissions `proposals:read` (lecteur) et `proposals:decide`
    (membre) ; MCP : lecture exposée, décision sous « sign-off » ;
  - code `proposal-decided` ;
  - test `tests/proposals.rs`.
- **P1** :
  - motif de rejet et « Corriger et accepter » (valeurs texte) dans
    l'UI ;
  - page « À valider » (menu Connaissance, `/proposals`, filtre par
    statut) ;
  - section des propositions ouvertes en tête de la page d'un objet ;
  - fiche `assistant-kb/proposals.md` ;
  - ligne au lexique (« Proposition », la garde refuse « suggestion de
    la machine »).
- **P2** :
  - `propose: true` sur une entrée de lien d'un mapping (le diff
    l'indique, l'écriture crée la proposition dans la même transaction) ;
  - option `propose` du nœud `link_upsert` (`msg.payload.confidence`
    gardé ; réponse `{proposed, proposal_id}`) ;
  - packs : `entreprises` 1.2.0 (sans `filiale_de.etat`),
    `entreprises_fr` 2.2.0 (Wikidata propose ses liens) et
    `presence_web` 1.1.0 (sans `a_pour_site.statut` ; le flow écrit à
    partir de 80 et propose de 40 à 79) ;
  - la section « Sites à valider » et ses deux routes
    `/web-presence/candidates` sont retirées : la boîte commune les
    remplace ;
  - migration 000047 (D376) : les liens en attente deviennent des
    propositions (empreinte `migration:<id>`, remplacée par une nouvelle
    proposition du même lien) et sont fermés ; les attributs d'état
    disparaissent ;
  - tests mis à jour : `partner_watch` (Wikidata accepté depuis la boîte,
    nœud qui propose), `pack_rules`.
- Reste :
  - P3 : dossiers et voix dans la même table ;
  - P4 : livré pour `link_upsert` (D377, voir plus haut) ; reste `object_upsert` et les imports ;
  - fait depuis : la même proposition reçue à nouveau avec des preuves
    (`evidence` du nœud `link_upsert`), encore ouverte, prend ces preuves
    et la nouvelle confiance ; la boîte affiche les preuves en texte ;
  - producteurs d'objets (`object_upsert` `propose`, entrée d'objet d'un
    mapping), à faire quand un cas réel le demande.
