# Porte de validation des agents (D833–D840)

| | |
|---|---|
| Statut | **Choix produit tranchés par Shan le 2026-11-08** (§2) |
| Lot de la roadmap | OP9 de `operational-platform.md` : validation générique (D563) |
| Repose sur | `proposals.md` (D367, D371–D377), `autonomous-agents.md` (D380, D381, D390), `agent-unification.md` (U6 : écritures d'un agent interne en propositions `tool_call`), `iam.md` (comptes de service) |
| Numérotation | **D833…D840**, lots **VG1…VG3** |

## 1. Constat

Depuis U6 (2026-11-07), chaque écriture d'un agent interne devient une proposition
`tool_call` qu'une personne accepte (rejouée avec ses droits) ou rejette. C'est le mode
`human` de D563, imposé à tout. D563 demande en plus trois modes, une liste d'opérations
jamais déléguées, une séparation demandeur / validateur et un disjoncteur.

Un agent hébergé ne parle à PneX que par la passerelle LLM : il n'écrit pas dans PneX, il n'est
pas concerné ici. La porte porte sur les **outils** des agents internes (registre de l'assistant),
qui sont déjà les opérations permises (liste blanche D380).

## 2. Choix de Shan (2026-11-08)

| Question | Réponse |
|---|---|
| Droits d'une écriture sans personne (autonome, ou acceptée par un validateur) | **Compte de service de l'agent** (celui que chaque agent a déjà, avec ses rôles IAM) |
| Modes livrés | **Les quatre** : autonome, personne, validateur, chaîne |
| Expiration d'une demande en attente | **Jamais** (le contrôle de version protège d'une écriture périmée) |
| Disjoncteur du validateur | **Débit + tout accepté** : plus de 30 décisions par heure, ou 20 acceptations d'affilée sans refus → les demandes suivantes vont à une personne, jusqu'au réarmement à la main |

## 3. Décisions

### D833 — Un mode par outil d'écriture, dans la fiche de l'agent

`AgentSpec.gate` associe un outil d'écriture à un mode : `human` (défaut, absent = `human`),
`autonomous`, `validator`, `chain`. Un outil de lecture n'a pas de mode. Un mode n'est permis
que sur un outil de la liste de l'agent. La fiche est versionnée : un run garde le mode de sa
version.

### D834 — Les droits : le compte de service de l'agent

Une écriture autonome, ou acceptée par le validateur, est rejouée par `tools::execute` avec
les droits du **compte de service de l'agent demandeur** (ses rôles IAM, son rôle d'agent),
comme une décision humaine l'est avec ceux de la personne. Mêmes validations, mêmes 409.
L'auteur affiché est `agent: <nom>`, la provenance `proposal:<id>`.

### D835 — Pas de second système : la proposition `tool_call` porte la porte

Chaque écriture reste une proposition `tool_call`, même en autonome (acceptée aussitôt par le
compte de service, motif `autonomous`), pour un journal des décisions complet (comme D377).
Colonnes ajoutées : `gate` (mode), `gate_stage` (`validator` en attente du validateur, `human`
en attente d'une personne), et la décision du validateur (agent, run, `accept` / `reject`,
motif, date). Une écriture autonome refusée par l'outil est rejetée (motif = le code de refus),
et le refus est rendu à l'agent.

### D836 — Le validateur est un run d'un agent interne désigné

`AgentSpec.validator_id` désigne l'agent qui valide (obligatoire dès qu'un outil est en
`validator` ou `chain`). La demande lui est donnée comme **donnée** (fichier d'entrée balisé
`<data>`, D381) : outil, arguments, agent et run demandeurs. Sa réponse finale commence par
`ACCEPT` ou `REJECT`, puis le motif. Il n'a jamais le droit de modifier la demande (D563,
règle 2). Une réponse illisible, un run en échec ou arrêté : la demande passe à une personne.

- `validator` : `ACCEPT` → exécution avec le compte de service du demandeur ; `REJECT` → rejet.
- `chain` : `ACCEPT` → la demande reste ouverte, marquée « validée par l'agent X », et une
  personne décide ; `REJECT` → rejet.

Une personne peut toujours décider avant le validateur : c'est plus strict, jamais moins.

### D837 — Séparation

Le validateur n'est ni l'agent demandeur, ni un agent ayant un run dans la chaîne (D716) du
run demandeur. Vérifié à l'enregistrement de la fiche (pas soi-même, un agent interne de
l'org) et au moment de valider (chaîne) ; sinon la demande va à une personne.

### D838 — Jamais délégable

Liste serveur, non modifiable par une fiche : un outil dont la permission relève de `iam`,
`secrets`, `governance`, `devices`, `controls` ou `ota`, ou dont le verbe est `delete` ou
`purge`, ne prend que le mode `human` (refus `not_delegable` à l'enregistrement, et repli sur
`human` à l'exécution si la liste change). Aujourd'hui aucun outil d'écriture de l'assistant
n'y tombe : la liste protège les outils à venir.

### D839 — Disjoncteur

Avant de soumettre une demande au validateur V : si V a décidé plus de 30 demandes dans
l'heure, ou accepté ses 20 dernières décisions sans un refus (depuis son dernier
réarmement), V est **déclenché** (`agents.validator_tripped_at`, motif `rate` ou
`all_accepted`) et la demande va à une personne. Ses demandes suivantes aussi, jusqu'au
réarmement (`POST /agents/{id}/validator/rearm`, `agents:write`) sur la fiche de V.

### D840 — Contenu lu = donnée ; écriture après une lecture non fiable

Un run qui a lu un contenu extérieur (outils `UNTRUSTED_TOOLS`, SEC-71) ne fait plus
d'écriture `autonomous` : elle passe à une personne. Le validateur et la chaîne restent
possibles (le validateur lit la demande, pas le contenu).

## 4. Lots

| Lot | Contenu |
|---|---|
| **VG1** | Fiche (`gate`, `validator_id`) et contrôles (outil, jamais délégable, validateur), migration 000134, mode `autonomous` avec le compte de service, repli D840, vue de la proposition enrichie, codes |
| **VG2** | Modes `validator` et `chain` : run du validateur, lecture de sa réponse, repli sur personne, séparation par chaîne, disjoncteur et réarmement |
| **VG3** | UI : modes sur la fiche de l'agent, validateur, bandeau de disjoncteur et réarmement ; boîte « À valider » (étape, décision du validateur) ; fiche `assistant-kb` ; e2e ; `break-ui` |

## 5. Sécurité

- R1 : org du principal ; le validateur et le demandeur sont de la même org (vérifié).
- R2 : la fiche se règle avec `agents:write` ; le réarmement aussi ; la décision humaine garde
  `proposals:decide`.
- D381 : la demande arrive au validateur balisée comme donnée ; rien de ce qu'il lit n'ouvre
  un droit. Il ne peut qu'accepter ou refuser ; l'exécution reste bornée aux droits du compte
  de service du demandeur et à l'empreinte de la demande.
- Audit : chaque décision (personne, compte de service, validateur) reste sur la proposition,
  avec l'agent qui l'a prise.

## 6. Avancement

(à remplir lot par lot)
