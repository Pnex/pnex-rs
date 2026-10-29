app-title = Enregistreur PNeX
tray-open = Ouvrir
tray-enabled = Enregistrement activé
tray-quit = Quitter

login-title = Connexion à PNeX
login-server = Serveur
login-email = E-mail
login-password = Mot de passe
login-submit = Se connecter
login-busy = Connexion…

main-signed-in = { $email } sur { $server }
main-sign-out = Se déconnecter
main-org = Organisation
main-enabled = Enregistrement activé
main-enabled-help = L'enregistreur capte l'audio pour PNeX tant que c'est activé.

test-title = Test de transcription
test-help = Enregistre { $secs } secondes au micro et les fait transcrire par le serveur. Rien n'est conservé.
test-profile = Profil par défaut : { $name } ({ $language })
test-advanced = Mode avancé : choisir le modèle de transcription
test-model = Modèle de transcription
test-run = Lancer le test ({ $secs } s)
test-recording = Parlez… { $secs } s
test-sending = Transcription sur le serveur…
test-result = Transcrit en { $ms } ms :
test-empty-text = (aucune parole reconnue)
test-no-model = Aucun modèle de transcription n'est enregistré dans cette organisation. Importez-en un dans PNeX : Médias › Flux › Modèles et profils.
test-no-valid-model = Aucun modèle de transcription de cette organisation n'a encore passé sa vérification. Lancez Vérifier sur un modèle dans PNeX : Médias › Flux › Modèles et profils.
test-no-default = Aucun profil par défaut dans cette organisation. Dans PNeX (Médias › Flux › Modèles et profils), ajoutez un profil de transcription ou cliquez sur Mettre par défaut, ou passez en mode avancé.
test-loading = Chargement…

err-unreachable = Serveur injoignable.
err-not-pnex = Cette adresse n'est pas un serveur PNeX.
err-incompatible = Ce serveur parle le contrat d'API { $contract } : installez l'enregistreur de la version du serveur.
err-bad-credentials = E-mail ou mot de passe incorrect.
err-session-expired = Session expirée : reconnectez-vous.
err-no-microphone = Aucun micro trouvé.
err-microphone = Erreur du micro : { $detail }
err-access-denied = Votre rôle dans cette organisation ne permet pas de lancer un test de transcription.
err-media-asr-model-invalid = Le modèle de transcription est absent ou n'a pas passé sa vérification.
err-asr-test-failed = Le serveur n'a pas pu transcrire l'extrait.
err-asr-model-not-found = Modèle de transcription introuvable.
err-server = Erreur du serveur { $status } ({ $code }).

trust-ca-title = Faire confiance au certificat de ce serveur ?
trust-ca-message = { $base } utilise un certificat signé par sa propre autorité (edge PNeX local). Vérifiez que l'empreinte ci-dessous correspond à celle affichée par le serveur (application web, Profil → À propos) avant de lui faire confiance.
trust-ca-fingerprint = Empreinte SHA-256
trust-ca-cancel = Annuler
trust-ca-accept = Faire confiance et se connecter

tray-discard = Arrêter et supprimer l'enregistrement
tray-recording = PNeX : appel en cours d'enregistrement
rec-title = Enregistrement
rec-unsupported = L'enregistrement des appels n'est disponible que sous Linux et Windows pour l'instant.
rec-off = L'enregistrement est désactivé : activez-le plus haut pour enregistrer les appels automatiquement.
rec-waiting = En attente d'un appel : une appli d'appel (Teams, Zoom, Meet dans le navigateur…) qui prend le micro lance l'enregistrement.
rec-recording = Enregistrement { $app } — { $time }
rec-manual = à la main
rec-consent = Prévenez votre interlocuteur que l'appel est enregistré.
rec-start = Enregistrer maintenant
rec-stop = Arrêter et envoyer
rec-discard = Arrêter sans envoyer
rec-sending = Envoi de { $name }…
rec-sent = { $name } envoyé : sa transcription a démarré dans PNeX.
rec-untranscribed = { $name } envoyé, mais sa transcription n'a pas démarré : { $error }
rec-pending = { $count ->
    [one] 1 enregistrement attend d'être envoyé.
   *[other] { $count } enregistrements attendent d'être envoyés.
}
rec-send = Envoyer maintenant
rec-name = Appel { $app } du { $date }
rec-name-manual = Enregistrement du { $date }
err-capture = Erreur d'enregistrement : { $detail }
err-media-analysis-no-asr = Aucun profil de transcription utilisable dans cette organisation : ajoutez-en un dans PNeX (Médias › Flux › Modèles et profils).
err-recording-unreadable = Le fichier de l'enregistrement est illisible.
