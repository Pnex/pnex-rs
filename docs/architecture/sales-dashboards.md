# Tableaux de bord commerciaux (D721–D727)

| | |
|---|---|
| Statut | **Validée par Shan le 2026-11-05** (§6). TB1, TB2 et TB3 codés, non commités, sauf le filtre par propriétaire et l'écran Équipe |
| Déclencheur | CRM3 attendait une spec. Demande de Shan (2026-11-05) : partir de ce que font les autres logiciels |
| Repose sur | `object-aggregates.md` (D579–D588), `crm.md` |
| Numérotation | **D721…D727**, lots **TB1…TB3** |

## 1. Ce que fait le marché

Relevé du 2026-11-05 dans la documentation de six CRM et de deux outils de prospection, sans essai des produits. Deux éditeurs n'ont été lus qu'en extraits. Conformément à la règle du dépôt, ils ne sont pas nommés ici.

| Présence | Indicateurs |
|---|---|
| Quasi universels | Taux de gain, pipeline pondéré, cycle de vente et temps par étape |
| Fréquents | Conversion par étape, opportunités bloquées, prévision contre objectif, activités par commercial, performance par source |
| Propres à un ou deux outils | Vélocité composite, couverture de pipeline, cascade d'évolution du pipeline, taux de report des clôtures |
| Prospection | Entonnoir contacté, délivré, répondu, intéressé, par campagne |

Interactions récurrentes : filtres globaux (période, propriétaire), clic vers les fiches, comparaison de périodes, données selon les droits, objectifs par personne ou par équipe. L'envoi planifié par mail existe chez certains, sans être général.

**Les définitions diffèrent.** Le taux de gain est gagnés / (gagnés + perdus) chez l'un, un ratio gagnés / perdus chez un autre, et plusieurs éditeurs ne documentent pas leur formule. Chaque chiffre affiché doit donc dire sa définition.

**Ce qui décide de la faisabilité :** le stock courant se lit à tout moment, mais la conversion et le temps par étape exigent un historique daté. Il ne se reconstitue pas après coup : plus il commence tard, plus ces chiffres arrivent tard.

## 2. Écart avec PneX (relevé dans le code)

| Indicateur | État | Manque |
|---|---|---|
| Pipeline par étape, en nombre et en montant | **Livré** (écran Pipeline) | — |
| Activités en retard par propriétaire | **Livré** (écran Chiffres) | — |
| Salons : conversion, CA gagné, ROI | **Livré** (écran Salons) | — |
| Taux de réponse aux relances | **Livré** (écran Chiffres) | — |
| Taux de gain, tous temps | Calculable (`ratio` et `where`, D580) | Un écran |
| Performance par source | Calculable (`opportunity.source`) | Un écran |
| Opportunités bloquées | Calculable (`last_activity`, `$today-<n>`) | Un écran |
| Taux de gain et CA signé **par période** | Non | Date de clôture : seule `expected_close` existe |
| Prévision pondérée | Non | Aucune métrique montant × probabilité (D580) |
| Signé contre objectif | Non | Aucun type d'objectif |
| Conversion et temps par étape | Non | Stream de transitions dans O2 (D722) |
| Motifs de perte | Non | `loss_reason` est un texte libre, non regroupable |
| Filtres de période et de propriétaire pour celui qui regarde | Non trouvé | Période : variables D509 ; propriétaire : voir §4 |

## 3. Décisions proposées

### D721 — Douze indicateurs, pas de constructeur en plus

On livre de bons défauts dans les écrans du pack, avec le moteur existant (D584, D588). Le pack garde ses écrans actuels et y ajoute ceux du §4.

Écartés pour l'instant : vélocité composite (sans définition commune), cascade et taux de report (journal des montants et des dates requis), détection d'anomalies, taux d'ouverture (peu fiable, et il exigerait un pixel de suivi dans les mails, contraire à l'esprit de D610).

### D722 — L'historique d'étape vit dans O2

Conforme à D184 : pas de table d'historique en base (décision de Shan, 2026-11-05, qui écarte une première proposition de table). Le journal `object_changes` contient déjà les changements d'étape, mais il range `before` et `after` en texte JSON, donc impossible à regrouper en SQL.

Option générique `history` sur une propriété d'énumération interne (`opportunity.stage`). Chaque changement ajoute une entrée à un stream O2 `object_transitions` : objet, type, propriété, ancienne valeur, nouvelle valeur, date. Il passe par la même file que le journal, donc par tous les chemins d'écriture : interface, flow, import. Seule une énumération est admise : aucune valeur libre, donc rien de personnel à effacer.

Limites acceptées, héritées du journal :

- **Écriture au mieux :** une entrée se perd si la file est pleine ou si O2 est indisponible. Ces chiffres sont des tendances, jamais une comptabilité. Le CA et le taux de gain ne s'y appuient pas (D723).
- **Rétention :** celle d'O2 (D72), à régler pour couvrir au moins deux ans, sinon le chiffre rétrécit sans bruit. Le widget indique sa période réelle.
- **O2 indisponible :** l'écran Passage par étape affiche « indisponible », les autres écrans restent lisibles.
- **Filtre par propriétaire :** le propriétaire est en base, pas dans O2. Le service résout d'abord les ids en base, puis interroge O2, avec un plafond à mesurer au lot TB3.
- **Pas de reprise pour l'instant :** le suivi commence à cette version, et l'écran le dit (« depuis le début du suivi »). Une reprise depuis `object_changes` reste possible plus tard, car le journal garde les avant et après.
- **Plafond :** 10 000 transitions lues, sur trois ans au plus ; au-delà, la réponse porte `truncated`. Pas encore de filtre par propriétaire. Une application lit ce chiffre avec `ontology:read`, pas par ses ensembles.

### D723 — Deux dates sur l'objet, en base

`closed_on` et `stage_entered_on` sont de l'état courant de l'opportunité, pas de l'historique : elles restent des propriétés, écrites par le serveur, en lecture seule pour les utilisateurs. `closed_on` est posée à l'entrée en `won` ou `lost` et retirée à la réouverture. Sans elle, « signé ce trimestre » n'existe pas : `expected_close` est une intention. Elles rendent le CA, le taux de gain par période et l'âge dans l'étape exacts, sans dépendre d'O2. Limite : une opportunité déjà clôturée avant cette version n'a pas de date, et une date historique d'un import est remplacée par celle du jour.

### D724 — Prévision : la probabilité de l'opportunité, sans valeur inventée

Option `weight` d'une métrique `sum` (D580), mêmes règles de devises. Une opportunité sans probabilité n'est pas comptée, et le widget dit combien sont dans ce cas. Pas de probabilité par défaut selon l'étape dans ce lot.

### D725 — Objectifs : un type du pack

Type `sales_target` : période (début, fin), montant, au niveau de l'org (§6). Saisi à la main, aucun calcul automatique de quota. Le flow « CRM · target progress » écrit chaque heure le CA gagné de la période, dans la devise de l'objectif, et la part atteinte : les chiffres n'avancent qu'une fois ce flow déployé.

### D726 — Chaque chiffre dit sa définition et son périmètre

- **Taux de gain :** gagnés / (gagnés + perdus), en nombre, sur les opportunités **clôturées dans la période**. Les ouvertes n'y entrent pas.
- **Conversion par étape :** part des opportunités entrées dans une étape qui ont atteint la suivante, sauts comptés comme passages. Les retours en arrière ne comptent pas deux fois.
- **Bloquée :** ouverte et sans activité depuis 30 jours, seuil modifiable dans le pack.
- Le widget affiche sa définition, et le clic sur un tableau ouvre les fiches concernées. Le clic sur une barre reste à faire.

### D727 — Motifs de perte : une énumération, le texte reste

`loss_category` (prix, concurrent, pas de budget, pas de décision, timing, autre) à côté de `loss_reason`. Valeurs à relire par Shan.

## 4. Écrans

| Écran | Contenu | Lot |
|---|---|---|
| Pipeline (existant) | Ajouter : liste des opportunités bloquées | TB1 |
| Résultats | Taux de gain, performance par source, signé par période, motifs de perte | TB1 pour le tous temps, TB2 par période |
| Prévision | Pondéré par mois de clôture prévue, signé contre objectif | TB2 |
| Passage par étape | Conversion et temps par étape | TB3 |
| Prospection | Entonnoir contacté, répondu, intéressé, par salon et par source | TB1, selon les données déjà présentes |
| Équipe | Signé et objectif par personne, derrière une permission dédiée (§6) | Après TB2, si des objectifs par personne sont demandés |

Filtres pour celui qui regarde : la **période** de l'écran Résultats est livrée (deux champs de date, 365 derniers jours par défaut, propres à sa session, D509). Le filtre par **propriétaire** n'est pas fait : il donnerait à tout lecteur des chiffres par commercial, que le §6 place derrière une permission dédiée et un avis juridique. Il suivra l'écran Équipe.

## 5. Lots

| Lot | Contenu | Dépend de |
|---|---|---|
| TB1 | Écrans sur les données existantes | Rien, et peut partir seul |
| TB2 | `closed_on`, `stage_entered_on`, `weight`, `sales_target`, `loss_category`, filtres de visionnage | Rien |
| TB3 | `history`, stream `object_transitions`, passage par étape | Rien ; à démarrer tôt, car le chiffre attend que l'historique s'accumule |

Le mail planifié n'est pas dans ces lots : un flow avec le nœud `ontology_aggregate` et une notification le permet déjà.

État de TB1 : Pipeline a gagné deux listes (bloquées, sans activité enregistrée), Résultats et Prospection existent, sur le tous temps. Depuis la version 3.7.0 du pack, le statut d'une piste a un historique (entonnoir `new`, `contacted`, `qualified`, `converted` ; `disqualified` arrête le chemin) : Prospection montre en tête les pistes ayant atteint chaque statut et la part passée au suivant, depuis le début du suivi, puis l'état courant par statut et par source (2026-11-05). Pas de reprise du passé : comme pour les étapes (D722). Reste à faire : « répondu » et « intéressé » n'existent pas comme statuts, le taux de réponse reste sur l'écran Chiffres, et l'entonnoir n'est pas encore découpé par salon ou par source. Limite : le clic vers les fiches existe sur les tableaux, pas encore sur les barres (D726).

## 6. Décisions de Shan (2026-11-05)

1. **Historique d'étape :** dans O2, pas en base (D722).
2. **Chiffres par commercial :** pas de classement ; signé et objectif par personne derrière une permission dédiée, sans comptage d'activités individuel par défaut. Suivre l'activité de salariés engage l'employeur : à faire confirmer par un juriste avant de livrer l'écran Équipe. Les activités en retard par propriétaire de l'écran Chiffres restent.
3. **Objectifs :** pour l'org d'abord. Le lien vers une personne de D725 attend l'écran Équipe.
4. **Bloquée :** 30 jours sans activité, pour toutes les étapes.

## 7. Sécurité

Lecture seule par la route d'agrégat, masquage et droits existants (D582). `history` n'écrit que des valeurs d'énumération internes, par le chemin d'écriture commun, dans le stream de l'org. Une seule route nouvelle, en lecture : `POST /ontology/transitions/query`, `ontology:read`, org du principal, clés confrontées au schéma avant la requête O2 (R1, R18). Grille `security.md` §6 parcourue le 2026-11-05 : deux constats, SEC-65 et SEC-66.

## 8. Décisions en attente de Shan (2026-11-05)

Aucune ne bloque le commit. Ce qui est en place reste le choix prudent indiqué.

| Sujet | Question | En place aujourd'hui |
|---|---|---|
| Filtre par propriétaire, écran Équipe | Permission dédiée et avis juridique : le lancer ? (§6) | Ni filtre ni écran |
| Clic sur une barre (D726) | Que doit-il ouvrir : un tableau filtré sur le même écran, ou une page de liste ? | Clic sur les tableaux seulement |
| Types de perte (D727) | Les valeurs conviennent-elles ? | `price`, `competitor`, `no_budget`, `no_decision`, `timing`, `other` |
| `meeting.invitation_uid` | Identifiant technique, ou donnée personnelle à effacer ? | Non personnel, commentaire dans `prospecting_b2b.yaml` |
| Reprise de l'historique d'étape | Remplir `object_transitions` depuis `object_changes` ? (D722) | Suivi à partir de cette version |
| Orgs déjà supprimées (SEC-66) | Nettoyer leurs journaux restés dans O2 ? | Non rattrapées |
| Journaux du serveur (SEC-65) | Masquer les valeurs recopiées avant de les envoyer à un outil externe ? | Sortie standard seulement |
| Installation lente sous charge | Réduire le nombre de requêtes ? Une transaction unique gagnerait peu (environ 226 synchronisations disque par test, mesuré) | Budget e2e de 60 s |
| Formulaire public de leads | Reporté par Shan : à quand ? | Non fait |
