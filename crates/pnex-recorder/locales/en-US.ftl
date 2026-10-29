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
