app-title = PNeX recorder
tray-open = Open
tray-enabled = Recording enabled
tray-quit = Quit

login-title = Sign in to PNeX
login-server = Server
login-email = Email
login-password = Password
login-submit = Sign in
login-busy = Signing in…

main-signed-in = { $email } on { $server }
main-sign-out = Sign out
main-org = Organization
main-enabled = Recording enabled
main-enabled-help = The recorder captures audio for PNeX while this is on.

test-title = Transcription test
test-help = Records { $secs } seconds from the microphone and has the server transcribe them. Nothing is stored.
test-profile = Default profile: { $name } ({ $language })
test-advanced = Advanced mode: choose the transcription model
test-model = Transcription model
test-run = Run the test ({ $secs } s)
test-recording = Speak now… { $secs } s
test-sending = Transcribing on the server…
test-result = Transcribed in { $ms } ms:
test-empty-text = (no speech recognized)
test-no-model = No transcription model is saved in this organization. Import one in PNeX: Media › Streams › Models and profiles.
test-no-valid-model = No transcription model of this organization has passed its check yet. Run Check on a model in PNeX: Media › Streams › Models and profiles.
test-no-default = No default profile in this organization. In PNeX (Media › Streams › Models and profiles), add a transcription profile or click Make default on one, or use the advanced mode.
test-loading = Loading…

err-unreachable = Server unreachable.
err-not-pnex = This address is not a PNeX server.
err-incompatible = This server speaks API contract { $contract }: install the recorder of the server's release.
err-bad-credentials = Wrong email or password.
err-session-expired = Session expired: sign in again.
err-no-microphone = No microphone found.
err-microphone = Microphone error: { $detail }
err-access-denied = Your role in this organization cannot run a transcription test.
err-media-asr-model-invalid = The transcription model is missing or has not passed its check.
err-asr-test-failed = The server could not transcribe the clip.
err-asr-model-not-found = Transcription model not found.
err-server = Server error { $status } ({ $code }).

trust-ca-title = Trust this server's certificate?
trust-ca-message = { $base } uses a certificate signed by its own authority (local PNeX edge). Check that the fingerprint below matches the one shown by the server (web app, Profile → About) before trusting it.
trust-ca-fingerprint = SHA-256 fingerprint
trust-ca-cancel = Cancel
trust-ca-accept = Trust and connect

tray-discard = Stop and discard the recording
tray-recording = PNeX: recording a call
rec-title = Recording
rec-unsupported = Call recording is available on Linux and Windows for now.
rec-off = Recording is off: switch it on above to record calls automatically.
rec-waiting = Waiting for a call: a call app (Teams, Zoom, Meet in the browser…) taking the microphone starts the recording.
rec-recording = Recording { $app } — { $time }
rec-manual = by hand
rec-consent = Tell the other party that the call is recorded.
rec-start = Record now
rec-stop = Stop and send
rec-discard = Stop and discard
rec-sending = Sending { $name }…
rec-sent = { $name } sent: its transcription has started in PNeX.
rec-untranscribed = { $name } sent, but its transcription did not start: { $error }
rec-pending = { $count ->
    [one] 1 recording waits to be sent.
   *[other] { $count } recordings wait to be sent.
}
rec-send = Send now
rec-name = Call { $app } { $date }
rec-name-manual = Recording { $date }
err-capture = Recording error: { $detail }
err-media-analysis-no-asr = No usable transcription profile in this organization: add one in PNeX (Media › Streams › Models and profiles).
err-recording-unreadable = The recording file cannot be read.
