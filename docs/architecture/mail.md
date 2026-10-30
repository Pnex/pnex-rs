# Boîtes mail : une source générique (D453–D462)

| | |
|---|---|
| Statut | **M1, M2, M6 codés — 2026-10-26**, écran `/mailboxes` le 2026-10-27 (menu Appareils, à côté des sites web) : serveur mail classique (IMAP, mot de passe) ; Microsoft reporté (§10). PneX **collecte et range** ; envoi (M3) codé le 2026-10-29, non commité (§9) |
| Déclencheur | Lot CRM3 de `crm.md` (mail entrant rattaché à la fiche), élargi : le mail est une source comme les flux et les sites web, utile bien au-delà du commercial |
| Repose sur | `iam.md` (principaux, comptes de service, équipes, D286–D299), `autonomous-agents.md` (agent = compte de service, D379, D381, D398), `secrets` (coffre D110–D118), `ontology.md` (types système D176–D177), `crm.md` (D442–D447 données personnelles), `security.md` (R8, R11–R13) |
| Numérotation | **D453…D463**, lots **M1…M6** ; envoi (M3) : **D518…D521** (2026-10-29) |

## 1. Le besoin en une phrase

Connecter des boîtes mail à PneX comme on connecte des flux : chaque boîte est une source configurée, ses nouveaux messages entrent, les flows et les agents les lisent, y répondent sous contrôle, et les relient aux objets de l'ontologie — sans écrire de code propre à un usage.

## 2. Les usages, pour vérifier que le modèle est générique

| Usage | Boîte | Qui la lit | Ce qu'on en fait |
|---|---|---|---|
| Commercial | `prenom@…`, la sienne | lui | mail ↔ contact, compte, opportunité (pack `crm`) |
| Boîte d'équipe | `sales@`, `contact@` | une équipe | même chose, partagée ; répartition |
| Relation client / SAV | `support@` | l'équipe SAV | un mail = une demande, rattachée au client et à l'équipement |
| Veille | une boîte d'abonnements (newsletters, alertes Google, BODACC) | personne en direct | flow : texte → thème de l'org → signal à valider (D335) |
| Agent IA | `agent-veille@…`, l'adresse de l'agent | l'agent (compte de service) | lire, préparer une réponse, la proposer ou l'envoyer selon sa politique |
| Fournisseurs, factures | `factures@` | la comptabilité | pièce jointe → média → extraction à valider |
| Machines | alertes envoyées par mail par un équipement tiers | un flow | mail → événement → notification |

Une seule chose varie d'un usage à l'autre : **qui possède la boîte et qui en lit le contenu**. Le reste — relève, stockage, déclencheur, envoi, garde-fous — est commun.

## 3. Décisions

### D453 — Une boîte mail est une source, gérée par un CRUD classique

Ressource `mailbox` de l'org, rangée dans le groupe de menu **Sources** (à côté des appareils, agents navigateur, sites web), liste harmonisée (tableau, pagination, bouton Ajouter) :

| Champ | Contenu |
|---|---|
| `name`, `address` | nom affiché, adresse de la boîte |
| `owner` | propriétaire (D454) |
| `incoming` | IMAP : hôte, port, TLS obligatoire, identifiant, secret du coffre ou jeton OAuth (D459), dossiers relevés (`INBOX` par défaut) |
| `outgoing` | SMTP, facultatif : sans lui, la boîte est en lecture seule |
| `state` | `active`, `paused`, `error` + `last_error` (code machine + args), `last_sync_at` |
| `retention_days` | durée de conservation des messages (défaut 90, D461) |
| `send_mode` | `draft` (défaut) ou `send` (D456) |

Le mot de passe ou le jeton ne quitte jamais le coffre (R4, R16) ; le formulaire suit D113 (choisir un secret ou en taper un, réservé owner/admin). **Tester la connexion** est une route à part (EXCLUDED du MCP : sortant).

### D454 — Le propriétaire est un principal ; le partage passe par les équipes

`owner` prend trois formes, sans nouveau type de principal :

| Forme | Exemple | Qui gère la configuration | Qui lit le contenu |
|---|---|---|---|
| `user` | la boîte d'un commercial | lui, et les admins (sans le contenu) | lui seul |
| `service_account` | la boîte d'un agent IA ou d'une application | les admins | le compte de service (ses runs), et les admins |
| `org` (partagée) | `sales@`, `support@`, veille | les admins | les équipes listées sur la boîte (`teams`) |

- Permissions : `mail:read` (voir que les boîtes existent, leur état), `mail:write` (gérer ses boîtes ; les boîtes partagées et de service : admin), `mail.content:read` (lire des messages, **en plus** d'être propriétaire ou dans une équipe de la boîte), `mail:send` (D456). Inscrites au registre IAM, appliquées par l'extracteur `OrgContext` (R2).
- **Un admin ne lit pas la boîte personnelle d'un membre** : il voit qu'elle existe, son état, peut la mettre en pause ou la supprimer. C'est une règle d'accès, pas une option.
- Un flow agit avec les droits de celui qui l'a déployé : il ne peut écouter que les boîtes dont son déployeur lit le contenu (vérifié au déploiement et à chaque message, comme D144).

### D455 — Les messages sont stockés, exposés comme type système `mail_message`

Deux possibilités ont été pesées :

- **A. Déclencheur seulement** : PneX ne garde que ce qu'un flow écrit. Simple, minimal en données, mais : rien à lister pour un flow ou un agent (« les nouveaux mails »), pas de fil de discussion, rien à rejouer après un redéploiement, l'historique client dépend d'un flow bien écrit.
- **B. Stockage + déclencheur** (retenu) : chaque message relevé est une ligne d'une table native, exposée dans l'ontologie comme **type système `mail_message`** (comme `media_asset` ou `doc`, D177).

Ce que B apporte sans code propre à un usage : la liste et la recherche (requête D185, `ontology_query` dans un flow), les liens vers les personnes, comptes, opportunités, demandes SAV (les packs les déclarent), le masquage par rôle et l'effacement (D442–D446 : expéditeur, destinataires et texte sont `personal`), la rétention (D461), le fil (`in_reply_to`, `references`).

Champs : boîte, dossier, `message_id`, `in_reply_to`, `references`, expéditeur, destinataires (plafonnés), sujet, date, texte (D458), extrait, résultat d'authentification (D458), pièces jointes (liste de noms et tailles en M2 ; médias en M5), `direction` (`in`/`out`), drapeaux (`auto_submitted`, `from_pneX`). Pas de HTML stocké.

Le contenu suit D454 : la ligne n'est visible que de qui lit la boîte (prédicat appliqué dans les requêtes, comme le futur I4).

### D456 — Lire, répondre, envoyer : un déclencheur, un nœud, une porte

- **Déclencheur `mail_received`** : nœud de flow source, config `mailbox` (une ou plusieurs) + filtres simples (dossier, expéditeur, sujet contient). Message : l'objet `mail_message` (id, champs D455). Fiche `NodeDoc`.
- **Lister** : `ontology_query` sur `mail_message` (non lus par le flow, d'une boîte, d'un fil) — aucun nœud de plus.
- **Répondre / envoyer** : nœud `mail_send` (à, sujet, texte, `in_reply_to`) et route `POST /mailboxes/{id}/messages`. Selon `send_mode` de la boîte :
  - `draft` (défaut) : le message devient une **proposition** (D367) dans « À valider » ; un humain relit et envoie ;
  - `send` : envoi direct, réservé aux boîtes partagées et de service, activé par un admin, plafonné (D457).
- **Agents IA** : leurs outils `mail_list`, `mail_read`, `mail_reply` passent par les mêmes services ; la politique de l'agent (`approval`, D380) peut imposer la proposition même si la boîte est en `send`. Le contenu d'un mail entre dans le contexte du run, jamais dans ses droits (D381).
- Le MCP expose la lecture (pas l'envoi : motif sortant).

### D457 — Pas de boucle

Une boucle naît quand un mail déclenche un envoi qui déclenche un mail. Garde-fous, tous au moteur :

1. **Réponses automatiques ignorées** pour le déclenchement d'un envoi : `Auto-Submitted` ≠ `no`, `Precedence: bulk | auto_reply | list | junk`, `X-Autoreply`, `X-Autorespond`, `List-Id` / `List-Unsubscribe` (listes), rapports de non-remise (`multipart/report`, `MAILER-DAEMON`, `postmaster`). Le message est stocké (drapeau `auto_submitted`), le déclencheur reçoit le drapeau ; `mail_send` refuse de répondre à un message drapé (`mail-reply-to-automatic`).
2. **Nos envois marqués** : tout envoi porte `Auto-Submitted: auto-generated` quand il vient d'un flow ou d'un agent, et `X-PneX-Origin: <org>/<boîte>/<id>` ; un message entrant qui porte notre marque, ou dont l'expéditeur est une boîte connectée de l'org, ne déclenche rien (`from_pneX`).
3. **Une seule entrée par `Message-ID`** et par boîte (clé naturelle).
4. **Plafonds** : envois par boîte et par heure (défaut 30), réponses au même correspondant par jour (défaut 3), messages relevés par boîte et par heure (défaut 500). Dépassement : la boîte passe `paused` avec le code (`mail-rate-exceeded`), visible et notifiable — jamais une tempête silencieuse.
5. **Pas d'envoi en chaîne** : un envoi déclenché par un `mail_received` porte la profondeur ; au-delà de 1, refusé.

### D458 — Le contenu d'un mail est une donnée non fiable

- **Jamais de HTML rendu** : on garde la partie `text/plain`, sinon le HTML converti en texte côté serveur (balises retirées, entités décodées). Rien n'est affiché comme HTML sur l'origine de l'application (R11–R13). Aucune image distante chargée, aucun lien suivi par un robot de PneX.
- **Bornes** : message brut ≤ 10 Mo relevé (au-delà : en-têtes seuls, `mail-too-large`), texte stocké ≤ 100 000 caractères, 100 destinataires, caractères de contrôle retirés, encodages normalisés en UTF-8.
- **Pièces jointes** : jamais ouvertes ni exécutées ; en M5 seulement, stockées comme médias sous la liste blanche de types existante, analysées à la demande.
- **Injection de prompt** : le texte passe aux modèles comme **donnée** (contrat de `llm_text`) ; aucun outil, aucune écriture, aucun envoi n'est décidé par le contenu ; tout ce qu'un modèle en tire est une proposition (D367) ou un brouillon.
- **Usurpation** : PneX vérifie lui-même l'expéditeur à la relève : une signature DKIM valide dont le domaine est aligné sur celui du `From:` (le même, un parent ou un sous-domaine) vaut DKIM et DMARC « pass » ; le SPF reste « none », une boîte ne connaît pas l'IP d'origine. L'en-tête `Authentication-Results` du message n'est **jamais cru** : un hébergeur qui n'en ajoute pas (cPanel, o2switch, constaté à l'essai du 2026-10-26) laisse l'expéditeur écrire le sien. Un message tronqué (au-delà de la taille plafond) reste non vérifié. Le verdict est stocké. Un message dont l'authentification échoue n'est **jamais rattaché automatiquement** à une personne ou un compte : le rattachement devient une proposition. Le déclencheur porte le résultat, les flows peuvent filtrer.
- **Connexion** : TLS obligatoire (IMAPS 993, SMTP 465 ou STARTTLS refusé s'il n'aboutit pas), certificat vérifié (CA système, comme le canal SMTP de `pnex-notify`), hôte passé au garde de sortie R8 (pas d'adresse interne, sauf politique `lan`).

### D459 — Connecteurs : IMAP/SMTP d'abord, OAuth ensuite

- **M2–M3 : IMAP + SMTP avec mot de passe d'application** (secret du coffre). Couvre les hébergeurs classiques, OVH, Gandi, Infomaniak, serveurs auto-hébergés, Gmail avec mot de passe d'application.
- **M4 : OAuth2 (XOAUTH2) sur les mêmes protocoles** pour Microsoft 365 et Google Workspace : Microsoft a coupé l'authentification par mot de passe en IMAP/SMTP sur Exchange Online, une boîte Outlook d'entreprise ne se connecte pas sans. Jeton de rafraîchissement au coffre, consentement par l'administrateur du tenant. Même pile de protocoles, pas d'API Graph à maintenir.
- Une API propriétaire (Graph, Gmail API) n'est ajoutée que si IMAP + XOAUTH2 ne suffit pas (Q-M4).

### D460 — Relève

- Un relève par boîte active, toutes les 2 minutes (singleton par boîte, sur une seule instance), curseur par dossier `(UIDVALIDITY, dernier UID)` ; un changement d'`UIDVALIDITY` relit les 7 derniers jours en s'appuyant sur `Message-ID` pour ne rien dupliquer.
- Le curseur n'avance qu'une fois la ligne écrite ; le déclencheur est servi depuis la table (un flow redéployé ne perd rien de ce qui est arrivé pendant son arrêt, dans la limite de 24 h).
- Erreur d'authentification ou TLS : `error` + code, relève suspendue jusqu'à une modification de la boîte (pas de martèlement d'un serveur qui refuse). Erreur réseau : nouvel essai avec délai croissant.
- IMAP IDLE (le push) : plus tard, même interface.

### D461 — Conservation et données personnelles

- `retention_days` par boîte (défaut 90, de 7 à 3650) : au-delà, le message est supprimé ; ce qu'un flow en a tiré (activité, signal) reste.
- Expéditeur, destinataires, sujet et texte sont `personal` (D442) : masqués par rôle, retirés par un effacement.
- Effacer une personne (D446) efface aussi les messages dont elle est l'expéditeur ou un destinataire (par ses adresses connues), en coquille.
- Une personne en opposition (D447) : ses messages entrent (on ne peut pas refuser un mail reçu), mais ne sont jamais rattachés automatiquement à elle ni à ses comptes.

### D462 — Ce que le pack `crm` en fait

Le moteur ne connaît que la boîte et le message. Le pack `crm` livre le flow modèle **« CRM · mails reçus »** : `mail_received` (boîtes choisies) → expéditeur retrouvé par son adresse (identifiant de la personne) → lien `activity_on` vers la personne et ses opportunités ouvertes, sauf authentification en échec ou opposition (alors proposition). Le pack de veille livre son propre flow ; le SAV le sien. Aucun d'eux n'ajoute de code au moteur.

### D463 — Un mail transféré est rangé sous son expéditeur d'origine

Un commercial dont la boîte n'est pas connectée (Outlook d'entreprise, D459) transfère vers une boîte relevée par PneX. Le message entrant a alors le commercial pour expéditeur ; PneX retrouve l'original :

- **message joint** (`message/rfc822`, transfert « en pièce jointe ») : ses en-têtes sont lus tels quels — le plus fiable ;
- sinon le **bloc de transfert du texte** : lignes « From / De / Von », « Date / Sent / Envoyé », « Subject / Objet », après un marqueur (« Forwarded message », « Message transféré », « Begin forwarded message », « Début du message réexpédié », « Original Message », une ligne de tirets ou de soulignés) ou sous un sujet « Fwd: / Fw: / TR: » ; formes Gmail, Outlook, Apple Mail et Thunderbird, français et anglais.

`forwarded = {from, date, subject, trusted}` est stocké avec le message. **Il n'est cru (`trusted`) que si** l'expéditeur du transfert est l'adresse d'un membre de l'org et que l'authentification du transfert passe (D458) : sans cette condition, n'importe qui pourrait déposer un faux « transfert » et faire ranger un mail chez un client. Un transfert de transfert (un client écrit à un collègue, qui transfère au commercial, qui transfère à la boîte) porte un bloc par maillon : la relève lit toute la chaîne (10 maillons au plus) et retient le **premier expéditeur extérieur à l'org**, les maillons écrits par des membres étant sautés ; si tous sont des membres, le plus récent. Constaté à l'essai du 2026-10-26 avec un transfert imbriqué de Gmail. Le rangement des packs (D462) lit `correspondent` : l'original d'un transfert cru, sinon l'expéditeur.

Une règle de **redirection** côté messagerie (qui garde l'expéditeur d'origine) n'a pas besoin de cette extraction ; elle reste soumise à l'authentification comme tout mail.

### D464 — Un nœud qui redémarre rejoue les mails manqués

La remise aux flows passe par une publication Valkey : un mail publié pendant qu'un flow est arrêté, en cours de redéploiement ou coupé du bus n'atteint jamais son nœud (constaté trois fois à l'essai du 2026-10-26). Chaque nœud `mail_received` garde un **curseur** en base (`mail_flow_cursors`, par flow et nœud, effacé avec le flow) : à chaque session (démarrage, redéploiement, reconnexion à Valkey), il s'abonne d'abord, puis demande au backend (`/internal/flow/mail/replay`, jeton de service et fencing comme les nœuds d'ontologie) les mails stockés après son curseur, filtrés par sa configuration **déployée** (les boîtes vérifiées au déploiement, D454), au plus 200 et des 7 derniers jours, du plus ancien au plus récent ; il accuse chaque mail émis (`/ack`), en direct ou rejoué, et le curseur n'avance que vers l'avant. Un mail croisé entre direct et rejeu n'est émis qu'une fois. Le premier déploiement pose le curseur à « maintenant » : pas d'historique déversé dans un flow neuf.

### D465 — Une pièce jointe devient un média à la demande d'un lecteur

Une pièce jointe n'est **jamais** copiée à la relève : un média est vu de toute l'org (droit `media:read`), un mail seulement de qui lit sa boîte (D454) ; copier d'office ferait fuir le contenu d'une boîte personnelle. Un **lecteur de la boîte** qui a aussi le droit d'ajouter des médias clique « Enregistrer dans les médias » (`POST /mailboxes/{id}/messages/{message}/attachments/{index}/media`, outil MCP du même nom) : c'est un partage délibéré, comme télécharger puis déposer le fichier.

- PneX ne garde aucun octet de pièce jointe : le serveur relit le message sous son UID (`EXAMINE` + `BODY.PEEK`, rien n'est marqué lu) et vérifie que c'est bien le message stocké (même `Message-ID`, même nom de pièce) ; sinon `mail-message-gone` (409), rien n'est enregistré.
- Les octets passent par la règle d'un dépôt : liste blanche de formats, extension conforme au contenu (`media-format-unsupported`), plafond de taille (`mail-too-large`) ; un message tronqué à la relève (au-delà de 10 Mo) est refusé.
- Le média est relié à la pièce (`media_id` dans la liste des pièces du message) : un second clic rend le même média tant qu'il existe. L'analyse (index, résumé) est celle de tout média, à la demande.
- Les droits : l'opération exige `media:write` (registre IAM) et la lecture de la boîte (`mail.content:read` et D454, sinon 404).

### D466 — Une invitation de réunion va au calendrier à la demande d'un lecteur

La partie `text/calendar` d'un mail (en ligne ou jointe) est lue à la relève par le lecteur iCalendar maison (`time_ranges::import::invitation`, jamais exécutée) : premier `VEVENT`, `UID`, `METHOD`, `STATUS`, début, fin (`DTEND` ou `DURATION`), lieu, organisateur, participants (plafonnés). Elle est gardée avec le mail (colonne `invitation`), visible de ses seuls lecteurs (D454).

- **Au calendrier sur demande** (comme D465, le calendrier est vu de toute l'org) : un lecteur de la boîte qui a `time-ranges:write` clique « Ajouter au calendrier » (`POST /mailboxes/{id}/messages/{message}/invitation/calendar`, outil MCP). La réunion devient une plage de l'org, catégorie `meeting`, clé = l'`UID` : ajouter une mise à jour la déplace, une annulation (`METHOD:CANCEL` ou `STATUS:CANCELLED`) la marque annulée (`attrs.status`).
- **Les gens** : organisateur et participants gardés dans `attrs.people` (rôle, nom, adresse) avec les personnes de l'org qui utilisent l'adresse comme identifiant e-mail (requête D185, masquage de l'appelant) ; personne n'est créé.
- **Aucune réponse** à l'organisateur (accepter, refuser) : il faudrait l'envoi (M3).
- Limite : une réunion passée s'affiche « en retard » au calendrier (état calculé pour la maintenance) tant qu'on ne la marque pas faite.

### D484 — Une invitation reçue devient un rendez-vous de l'ontologie, avec ses participants connus

Le calendrier (D466) reste à la demande d'un lecteur ; l'ontologie, elle, est remplie par un flow du pack `crm` (D462 : rien au moteur), **CRM · réunions reçues**, déployé par un membre sur les boîtes qu'il lit :

- chaque mail qui porte une invitation (hors `REPLY`, qui ne dit que la réponse d'un participant) écrit un `meeting` (pack `prospecting_b2b` 1.7.0) clé = `invitation_uid` (l'`UID`) : date, objet, lieu, statut `planned` ; une mise à jour déplace le même objet, une annulation (`METHOD:CANCEL`) le passe `cancelled`, l'historique de l'objet garde le reste ;
- l'organisateur et chaque participant dont l'adresse est l'identifiant e-mail d'une personne de l'org **participent** (`takes_part_in`) ; lien écrit quand l'expéditeur est authentifié (règle de `crm_mail_prep`, D458), **proposé** sinon (page « À valider », confiance 40) ;
- une adresse inconnue est laissée de côté : personne n'est créé (même règle que D466, D467).

Limites : une annulation envoyée à un seul participant retiré de la réunion (même `UID`) passe aussi le rendez-vous `cancelled` ; une invitation renvoyée après la réunion la remet `planned`. Le transfert d'une invitation Outlook a un autre `UID` : c'est un second rendez-vous. Une « puce » d'événement Gmail n'est pas une invitation (texte localisé, sans participants ni identifiant) : seule son invitation jumelle, envoyée par Google Agenda aux invités, est lue.

## 4. Ce que le système ne fait pas

- Pas de client mail complet (dossiers, règles, signatures) : PneX relève, relie et répond sous contrôle ; on lit ses mails dans sa messagerie.
- Pas d'envoi de masse ni de campagne (marketing, newsletters).
- Pas de lecture d'une boîte personnelle par un admin.
- Pas de rendu HTML, d'ouverture de pièce jointe ni de suivi de lien ; une pièce jointe n'est copiée en média qu'à la demande d'un lecteur (D465).

## 5. Sécurité (grille `security.md` §6)

| Point | Réponse |
|---|---|
| 1 Routes | CRUD, test de connexion, envoi : permissions D454 au registre IAM, org du principal ; tests 403 / 404 cross-org |
| 2 Données renvoyées | jamais de mot de passe ni de jeton ; contenu filtré par D454 |
| 3 Nœuds | `mail_received`, `mail_send` en liste blanche ; boîte vérifiée au déploiement contre le déployeur |
| 4 Sortant | IMAP/SMTP derrière R8, TLS obligatoire ; envoi = R9 (secret lié à sa boîte) |
| 5 Rendu | texte seul (D458) ; une pièce jointe enregistrée (D465) suit la règle des dépôts : liste blanche, type servi sûr et en-têtes de contenu utilisateur (SEC-5) |
| 6 Requêtes | paramètres liés ; dossiers IMAP échappés (pas de nom de dossier tapé tel quel dans une commande) |
| 7 Crypto, jetons | jetons OAuth au coffre, rafraîchis côté serveur |
| 8 Dépendances | client IMAP (`async-imap`, MIT/Apache-2.0) et analyse MIME (`mail-parser`, Apache-2.0/MIT) : vérifiés (maintenus, licences permissives), `task security:deps` au lot M2 |

## 6. Lots

| Lot | Contenu | Sortie |
|---|---|---|
| M1 | CRUD des boîtes, propriétaire et équipes, secret, test de connexion | une boîte s'ajoute et se teste |
| M2 | Relève IMAP, table + type système `mail_message`, déclencheur `mail_received`, garde-fous D457.1–3 et D458 | un flow reçoit les nouveaux mails |
| M3 | Envoi SMTP, `mail_send`, brouillons à valider, marquage, plafonds | répondre sous contrôle |
| M4 | OAuth2 Microsoft 365 et Google Workspace | boîtes Outlook d'entreprise |
| M5 | Pièces jointes en médias, outils de l'assistant et des agents | factures, agents qui lisent et répondent |
| M6 | Flows modèles des packs (`crm`, veille, SAV) | mail rattaché à la fiche (CRM3) |

## 7. Questions à trancher

1. **Q-M1 — Stockage** : B (stocker + déclencher) est recommandé ; A seulement si l'on refuse de garder des mails dans PneX.
2. **Q-M2 — Boîte personnelle** : l'admin voit-elle seulement qu'elle existe (recommandé), ou peut-il aussi lire, sur décision explicite de l'org ?
3. **Q-M3 — Envoi direct** (`send`) : autorisé pour les boîtes partagées et d'agents (recommandé, désactivé par défaut), ou brouillon toujours obligatoire ?
4. **Q-M4 — Microsoft 365 en premier ?** Si les premiers clients sont sur Outlook d'entreprise, M4 passe avant M3.
5. **Q-M5 — Conservation par défaut** : 90 jours ?

## 8. Récapitulatif des décisions

| # | Décision |
|---|---|
| D453 | La boîte mail est une source, CRUD classique |
| D454 | Propriétaire = un principal (utilisateur, compte de service, org) ; partage par équipes ; l'admin ne lit pas une boîte personnelle |
| D455 | Messages stockés, type système `mail_message` |
| D456 | Déclencheur `mail_received`, nœud `mail_send`, brouillon à valider par défaut |
| D457 | Garde-fous contre les boucles |
| D458 | Contenu non fiable : texte seul, bornes, pas de pièce jointe ouverte, expéditeur vérifié par PneX (DKIM aligné, crate `mail-auth`), jamais par l'en-tête du message |
| D459 | IMAP/SMTP puis OAuth2 (XOAUTH2) |
| D460 | Relève par curseur UID, reprise, erreurs |
| D461 | Conservation par boîte, effacement et opposition |
| D462 | Les packs livrent leurs flows ; rien au moteur |
| D463 | Un transfert d'un membre est rangé sous l'expéditeur d'origine |
| D464 | Un nœud `mail_received` qui redémarre rejoue les mails stockés après son curseur (200 au plus, 7 jours), jamais deux fois ; le premier déploiement part de maintenant |
| D465 | Une pièce jointe devient un média seulement à la demande d'un lecteur de la boîte qui peut ajouter des médias ; relue sur le serveur, vérifiée, passée par la liste blanche des dépôts |
| D466 | Une invitation de réunion lue à la relève va au calendrier de l'org à la demande d'un lecteur ; clé = `UID` (mise à jour, annulation), participants rattachés aux personnes par identifiant e-mail |
| D467 | Un expéditeur inconnu d'un domaine propre est proposé (jamais lié) sous le compte dont le site web confirmé a ce domaine ; webmails exclus |
| D484 | Une invitation reçue devient un `meeting` clé = `UID` (flow du pack `crm`), ses participants connus y participent (proposé si l'expéditeur n'est pas authentifié) ; annulation = statut `cancelled` ; personne n'est créé |
| D518 | Serveur SMTP sur la boîte (mêmes identifiants qu'IMAP, TLS implicite sur 465, STARTTLS exigé ailleurs, garde de sortie) ; mode `draft` par défaut, `send` sur boîte partagée ou de service seulement |
| D519 | Les brouillons vivent dans la boîte d'envoi de la boîte, lue avec D454, pas dans « À valider » (revient sur D456) |
| D520 | Garde-fous d'envoi au serveur : réponse à un mail automatique ou envoyé par PneX refusée aux flows, plafonds, pause d'une boîte qui envoie seule, `Auto-Submitted` seulement sans humain |
| D521 | Permission `mail:send` (Membre) ; liste de la boîte d'envoi au MCP, envoi et abandon exclus |

## 9. État

### 2026-10-26 — M1, M2 et M6 codés (non commité)

Arbitrages pris par défaut (recommandations du §7, réversibles) : messages stockés (Q-M1), boîte personnelle illisible par l'admin (Q-M2), pas d'envoi (Q-M3 sans objet tant que M3 n'est pas fait : PneX collecte et range), Microsoft reporté (Q-M4, §10), 90 jours (Q-M5).

| Brique | Où | Tests |
|---|---|---|
| Règles pures : drapeaux anti-boucle, verdict d'authentification, HTML → texte, bornes, transferts (D457, D458, D463), filtres du déclencheur | `pnex-core` `mail.rs` | 9 unitaires |
| Propriétaire = un `user_id` (personne ou compte de service, qui est un utilisateur) ou aucun (partagée, lue par ses équipes) ; permissions `mail:read`, `mail:write`, `mail:delete`, `mail.content:read`, `mail.shared:write` | `services/mail/mod.rs`, registre IAM | `tests/mail.rs` |
| Mot de passe au coffre (consommateur `mailbox`) : un membre tape celui de **sa** boîte, ne choisit jamais un secret ; changer de serveur exige de le retaper (R9) | `services/secrets/mailbox.rs` | idem |
| Client IMAP (IMAPS, CA système, `ring`, garde de sortie, `EXAMINE` + `BODY.PEEK`, rien n'est marqué lu) | `services/mail/imap.rs` (`async-imap` 0.12) | unitaires (dossiers) |
| Analyse MIME (texte seul, pièces listées, transfert joint ou dans le texte) | `services/mail/parse.rs` (`mail-parser` 0.11.9) | 3 unitaires |
| Relève toutes les 2 min, singleton, curseur `(UIDVALIDITY, UID)`, 7 jours au premier passage, 500 messages/heure puis pause, rétention, erreurs permanentes vs réseau | `services/mail/poller.rs` | `tests/mail.rs` (serveur injoignable) |
| Tables `mailboxes`, `mail_cursors`, `mail_messages` (unique par boîte × `Message-ID`) | migration 000058 | — |
| Routes `GET/POST /mailboxes`, `PUT/DELETE /mailboxes/{id}`, `POST …/state`, `POST …/test`, `GET …/messages` ; MCP : liste, messages, pause exposés, le reste exclu (secret, destructif, sortant) | `controllers/mailboxes.rs` | `tests/mail.rs`, gardes OpenAPI/IAM/MCP |
| Nœud `mail_received` (canal Valkey `pnex:mail:v1:{org}`), contrôle au déploiement (D454), éditeur de flow (palette Déclencheurs, inspecteur) | `pnex-core` `flow/*`, `pnex-node-ui-control/src/mail.rs`, `controllers/flows/lifecycle.rs`, front `flow_editor` | runtime 2 unitaires ; déploiement refusé sur la boîte d'un autre |
| Flow modèle **CRM · mails reçus** (D462) | `packs/crm.yaml` | `pack_rules.rs` |
| Effacement d'une personne → ses mails (D461) | `services/profiles.rs` | — |

Reste : l'écran de la liste des boîtes (CRUD) et le test réel sur une boîte jetable o2switch ; `mail_message` comme type système de l'ontologie (D455, liens et recherche depuis l'ontologie) ; l'envoi (M3) ; OAuth (M4, §10) ; les pièces jointes en médias (M5).

### 2026-10-29 — M3 : envoi (D518–D521), non commité

Demande de Shan (2026-10-29), avec le choix recommandé de Q-M3 : envoi direct autorisé pour les boîtes partagées et de service, désactivé par défaut.

**D518 — SMTP sur la boîte, mode d'envoi.** La boîte gagne `smtp_host` (vide = elle n'envoie pas), `smtp_port` (465 par défaut) et `send_mode`. Mêmes identifiant et mot de passe qu'IMAP (le cas des hébergeurs classiques). Le serveur et chaque adresse résolue passent la garde de sortie (R8), puis la connexion se fait sur l'adresse retenue, le certificat vérifié sur le nom (TLS implicite sur 465, STARTTLS exigé sur tout autre port, jamais en clair). Changer le serveur SMTP exige de retaper le mot de passe (R9, comme IMAP). `send_mode` : `draft` (défaut) ou `send` ; `send` refusé sur la boîte personnelle d'un membre (`send_mode` invalide), donc posé par un gestionnaire des boîtes partagées. Une boîte en pause ne prend rien des flows.

**D519 — La boîte d'envoi, pas « À valider ».** D456 voulait un brouillon en proposition D367. Mais la boîte « À valider » est lue par quiconque décide des propositions : le brouillon d'une boîte personnelle y serait lu par d'autres, contre D454. Chaque boîte a donc sa **boîte d'envoi** (`mail_outbox`), lue avec les règles de la boîte : brouillons des flows, envoyés, échoués, écartés. Un lecteur relit, corrige le sujet ou le texte, envoie ou écarte. Un envoi réclame d'abord la ligne (`sending`) : deux clics, ou un clic et un flow, n'envoient jamais deux fois. Un échec garde le mail (`failed`, avec l'erreur), à renvoyer. La clé (`key`) de l'appelant rend l'écriture idempotente.

**D520 — Garde-fous, au serveur.** Le runtime n'est pas cru : la profondeur de D457.5 devient une règle sur `in_reply_to`, qui doit nommer un mail stocké de la boîte ; un flow qui répond à un mail `automatic` est refusé (`mail-reply-to-automatic`), à un mail `from_pnex` aussi (`mail-send-chained`). Un membre peut répondre à ce qu'il veut. Plafonds (D457.4) : 30 envois par boîte et par heure pour tous ; pour un mail qui part sans humain, 3 par correspondant et par jour ; 100 brouillons de flows par boîte et par heure. Plafond atteint : sans humain, le mail échoue et la boîte passe en pause (`mail-send-limit`) ; envoyé par un membre, 429 et le brouillon reste. En-têtes : `X-PneX-Origin` toujours (le mail revenu ne déclenche rien), `Auto-Submitted: auto-generated` seulement sans humain (un brouillon relu puis envoyé par un membre est son mail), `Message-ID` sur le domaine de la boîte, `In-Reply-To` et `References` pour le fil. Texte seul.

**D521 — Droits et exposition.** Nouvelle permission `mail:send` (Membre) pour écrire, envoyer et écarter, en plus de lire la boîte (`mail.content:read`, D454). Nœud `mail_send` : la boîte doit être lue par la personne qui déploie (même garde que `mail_received`) ; le runtime ne porte ni boîte ni mot de passe, il appelle `POST /internal/flow/mail/send` qui relit la config déployée. MCP : liste de la boîte d'envoi exposée ; écriture et envoi exclus (sortant), abandon exclu (destructif).

| Brique | Où | Tests |
|---|---|---|
| Types, contrôles (destinataires, sujet sans saut de ligne, bornes), config du nœud | `pnex-core` `mail.rs` | 2 unitaires |
| Colonnes SMTP, table `mail_outbox` | migration 000065 | — |
| Service : écriture, liste, envoi réclamé, abandon, plafonds, construction du message, session SMTP (`lettre`, rustls, CA système) | `services/mail/send.rs` | 3 unitaires (en-têtes, fil, marque reconnue au retour) |
| Routes `POST /mailboxes/{id}/messages`, `GET …/outbox`, `POST …/outbox/{mail}/send`, `POST …/outbox/{mail}/discard` ; IAM, exposition, contrat | `controllers/mailboxes.rs`, `pnex-api-contract` | `tests/mail.rs` `a_mail_waits_in_the_outbox_until_a_reader_sends_it` |
| Nœud `mail_send` (2 ports), garde de déploiement | `pnex-core` `flow/*`, `pnex-node-ui-control/src/mail_send.rs`, `controllers/internal_flow.rs`, `controllers/flows/lifecycle.rs` | 1 unitaire ; `tests/mail.rs` (déploiement refusé sur la boîte d'un autre) |
| Écran : SMTP et mode dans le formulaire, boîte d'envoi sur la page de la boîte, nœud dans l'éditeur | `pnex-frontend` `pages/mailboxes.rs`, `pages/mailbox_outbox.rs`, `flow_editor/*` | compilation |

Limites : **pas essayé contre un vrai serveur SMTP** (le test couvre un serveur injoignable) : l'essai sur la boîte o2switch reste à faire. Le mail envoyé n'est pas copié dans le dossier « Envoyés » du serveur (pas d'`APPEND` IMAP) : il n'apparaît que dans la boîte d'envoi de PneX (à ajouter si les utilisateurs le réclament). Pas de pièce jointe, pas de HTML. Le bouton Tester ne teste pas encore SMTP. Restent l'outil `mail_reply` des agents (attend D380), Microsoft (M4) et le branchement du pack `crm` (brouillon de relance → boîte d'envoi).

### 2026-10-27 — M5, premier volet : pièces jointes en médias (D465)

Service `services/mail/attachments.rs` (relecture IMAP, contrôle du message, dépôt par `write_version`), extraction `parse::attachment`, route `POST …/messages/{message}/attachments/{index}/media` (IAM `media:write`, exposée au MCP), bouton « Enregistrer dans les médias » / « Ouvrir le média » dans l'écran des mails, fiche `assistant-kb/mailboxes.md`. Tests : `tests/mail.rs` (non-lecteur 404, pièce absente 404, serveur relu 502, média déjà enregistré rendu sans serveur), 1 unitaire de décodage. Non essayé sur la boîte o2switch réelle. Outils de l'assistant (et des agents, même registre) : `list_mailboxes`, `list_mails`, `read_mail` en lecture seule (`services/ai/mail_tools.rs`, registre IAM `mail:read` / `mail.content:read`, règle D454 du service, texte et sujet entre balises `<data>`), test `tests/mail.rs` (un admin ne lit pas la boîte d'un membre). Reste : `mail_reply`, qui attend M3.

## 10. Reprise : boîte jetable d'abord, Microsoft à contourner

**Tests** : une boîte jetable chez un hébergeur classique (o2switch), abonnée à des newsletters et des alertes. IMAP + mot de passe y fonctionnent ; c'est le cas de la veille (§2) et il suffit à valider M1–M3 sans dépendre d'un éditeur.

**Microsoft 365 : à inventer, sans alourdir le moteur.** Exchange Online refuse le mot de passe en IMAP/SMTP ; une connexion demande OAuth2, donc :

- une **application déclarée dans Entra ID** (par l'admin du tenant du client, ou une application multi-tenant publiée par PneX que l'admin approuve une fois) ;
- un **consentement de l'utilisateur** à la première connexion de sa boîte : redirection vers Microsoft puis retour vers PneX, ou, sans redirection, le **flux « device code »** (l'utilisateur tape un code sur `microsoft.com/devicelogin`), mieux adapté à un PneX auto-hébergé ;
- un **jeton de rafraîchissement** gardé au coffre et renouvelé côté serveur ; à refaire seulement s'il expire ou est révoqué ;
- pour une **boîte partagée**, des droits délégués ou « application » accordés par l'admin du tenant.

Les gestes manuels sont donc concentrés au branchement d'une boîte (une fois par utilisateur, et une fois pour l'admin du tenant), pas à l'usage.

**Pistes à étudier avant d'écrire du code propre à Microsoft :**

1. **Un serveur MCP tiers** déjà fait (ex. `ms-365-mcp-server` de Softeria : lister, lire, envoyer, brouillons, boîtes partagées via Graph, connexion par device code), appelé par les agents et l'assistant comme un outil externe. PneX ne porterait ni le protocole ni l'OAuth Microsoft ; à vérifier : maintenance, licence, sécurité des jetons, place dans la frontière D381 (le contenu reste une donnée).
2. **Device code + XOAUTH2 sur IMAP/SMTP** : un seul écran (« saisissez ce code »), même pile que les autres boîtes.
3. **Transfert automatique** : une règle Outlook qui transfère vers une boîte IMAP ordinaire (sans OAuth côté PneX ; à vérifier : politique anti-transfert externe des tenants, perte des fils).

Aucun service officiel Microsoft n'est apparu à la recherche du 2026-10-26 ; les serveurs MCP trouvés sont communautaires ou hébergés par des tiers.

## 11. Piste (2026-10-27) : invitations de réunion et reconnaissance des correspondants

**Point 1 livré (D466)**, choix par défaut : calendrier de l'org (pas de calendrier par personne aujourd'hui), ajout à la demande.

**Point 2 livré (D467, pack `crm` 1.4.0)** : le domaine d'une entreprise n'est pas saisi, il vient de son site web confirmé (pack `web_presence`, `website.domain`, lien `has_website`). Le flow **CRM · mails reçus** cherche d'abord une personne par identifiant e-mail ; s'il n'y en a aucune et que l'adresse n'est pas un webmail, il range le mail comme activité et **propose** son rattachement au compte dont le site a ce domaine, et à ses opportunités ouvertes (page « À valider », confiance 50, preuve = le domaine). Jamais lié d'office, même expéditeur authentifié : un clic humain valide, comme les sites candidats de la veille. Point 3 (LinkedIn) ouvert.

Demande : une invitation reçue ou transférée par mail apparaît au calendrier avec ses participants, et chaque correspondant est relié à sa personne, à son entreprise (par le domaine de son adresse) et, le cas échéant, à son profil LinkedIn.

Ce qui existe déjà et servirait :

- l'analyse MIME voit la partie `text/calendar` (aujourd'hui seulement listée en pièce jointe) ;
- un analyseur iCalendar maison (`services/time_ranges/import.rs` : `VEVENT`, `UID`, `DTSTART`/`DTEND`, `TZID`) ; l'`UID` sert de clé, donc une invitation mise à jour remplace l'ancienne ;
- le calendrier (plages, `list_calendar`) ;
- le rattachement du mail à la personne par son adresse (flow **CRM · mails reçus**), les propositions « À valider » quand l'expéditeur n'est pas vérifié (D458).

À construire (lot M7 possible) :

1. **Invitation → plage du calendrier** : `METHOD:REQUEST` crée ou met à jour, `CANCEL` annule ; organisateur et `ATTENDEE` rattachés aux personnes de l'org par adresse, inconnus proposés (jamais créés d'office, D458) ; aucune réponse (accepter/refuser) envoyée, il faudrait M3.
2. **Entreprise par domaine** : le domaine de l'adresse rapproché du domaine du compte (champ à ajouter au type `account` du pack `crm`) ; liste d'exclusion des messageries publiques (gmail.com, outlook.fr, orange.fr…) ; rapprochement = proposition quand il n'est pas unique.
3. **LinkedIn** : pas de collecte automatique sur LinkedIn (conditions d'utilisation, données personnelles) ; un champ « profil » tapé par un humain, ou une proposition tirée d'une recherche web (fournisseur de recherche de l'org) qu'un humain valide.

Questions : l'invitation va-t-elle au calendrier de la boîte, ou à celui de chaque participant interne ? Le domaine d'entreprise est-il saisi à la main ou déduit de son site web ?
