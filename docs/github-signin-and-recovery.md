# GitHub sign-in and password recovery (owner guide)

Hagency Desktop's login screen offers three account options that Pasion
(the Hagency auth service) provides:

- **Continue with GitHub** — shown when the server advertises an upstream
  provider whose `brand_name` is `github`. The desktop sends
  `login_hint=upstream:github`, so Pasion skips its chooser and opens GitHub.
- **Forgot password?** — sends a reset link (and the username) to the email on
  the account.
- **Email at sign-up (optional)** — when given, Pasion emails a code the user
  types into the sign-up form. Without an email the account cannot be
  recovered by email.

## 1. Create a GitHub OAuth app per server

GitHub > Settings > Developer settings > OAuth Apps > New OAuth App.

| Field | alpha | beta |
|---|---|---|
| Homepage URL | the server's public URL | the server's public URL |
| Authorization callback URL | `<alpha public URL>/upstream/callback/01M7BKN7F5NRSNBNR7P3CEDYGR` | `<beta public URL>/upstream/callback/01M7BKN7F5H6QQ0XP4QWC6APFV` |

The provider IDs are fixed (also in `hagency-server/.run/github-provider-ids`);
the callback URL must use exactly that ID.

Store each client secret in a file only you can read:

```sh
mkdir -p ~/.hagency-secrets && chmod 700 ~/.hagency-secrets
printf '%s' '<alpha client secret>' > ~/.hagency-secrets/github-alpha.secret
printf '%s' '<beta client secret>'  > ~/.hagency-secrets/github-beta.secret
chmod 600 ~/.hagency-secrets/github-*.secret
```

Never commit secrets or paste them into config files; use `client_secret_file`.

## 2. Pasion provider config

Added to each instance's `pasion.toml` (`hagency-fed-up.sh` writes it when the
secret file exists and `<name>_client_id=` is recorded in
`hagency-server/.run/github-provider-ids`; `GITHUB_CLIENT_ID_<NAME>` overrides it.
Without both, no provider is written and the button stays hidden):

```toml
[[upstream_oauth2.providers]]
id = "01M7BKN7F5NRSNBNR7P3CEDYGR"         # beta: 01M7BKN7F5H6QQ0XP4QWC6APFV
brand_name = "github"
human_name = "GitHub"
client_id = "<client id>"
client_secret_file = "/Users/<you>/.hagency-secrets/github-alpha.secret"
token_endpoint_auth_method = "client_secret_post"
discovery_mode = "disabled"
pkce_method = "never"
fetch_userinfo = true
authorization_endpoint = "https://github.com/login/oauth/authorize"
token_endpoint = "https://github.com/login/oauth/access_token"
userinfo_endpoint = "https://api.github.com/user"
scope = "read:user user:email"
claims_imports.subject.template = "{{ userinfo_claims.id }}"
claims_imports.localpart = { action = "suggest", template = "{{ userinfo_claims.login }}" }
claims_imports.displayname = { action = "suggest", template = "{{ userinfo_claims.name }}" }
claims_imports.email = { action = "suggest", template = "{{ userinfo_claims.email }}" }
```

## 3. Email delivery (recovery and sign-up codes)

Recovery and sign-up codes need a working mail provider; the default
(`blackhole`) drops mail. Local demo: run Mailpit and point Pasion at it:

```toml
[email]
from = '"Hagency" <no-reply@hagency.local>'
reply_to = '"Hagency" <no-reply@hagency.local>'
[email.provider]
type = "smtp"
mode = "plain"
hostname = "127.0.0.1"
port = 1025
[account]
password_recovery_enabled = true
```

Mailpit's inbox is at http://127.0.0.1:8025. `hagency-fed-up.sh` starts it in
Docker when `HAGENCY_MAILPIT=1`. For production use an SMTP relay with TLS or
one of the API providers (Resend, SendGrid, Brevo, AWS SES).

## 4. Check it

1. Restart: `./hagency-fed-up.sh --restart`.
2. Desktop > pick the server: "Continue with GitHub" appears.
3. "Create an account" with an email: a code arrives in Mailpit.
4. "Forgot password?" with that email: a reset link arrives in Mailpit.
