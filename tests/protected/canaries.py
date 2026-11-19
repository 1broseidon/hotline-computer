"""The made-up secrets this suite delivers. Nothing here is real: every value
is a canary invented for the probes, and the passkey is a throwaway P-256 key."""

import json

VAR = "hl-canary-7f3a"                      # CANARY_VAR, a plain variable
GH = "ghp_hlCanaryW5Token0000000000000000000"  # GH_TOKEN, a key-shaped variable
PASSWORD = "hl-canary-pw-91c2"              # EXAMPLE.password
ASK_PASSWORD = "hl-canary-ask-55e0"         # ASKME.password
LOCAL_PASSWORD = "hl-canary-local-3b8d"     # LOCALAPP.password, on http://localhost:8000
COOKIE = "hl-canary-cookie-w5-8c41"         # a signed-in session cookie on example.test
TOTP_SEED = "JBSWY3DPEHPK3PXPJBSWY3DP"

_KEY = ("MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgB9aEvdD1k7n55Wu+sDtf5cUbUumdw/"
        "XyKmgddnpO1eihRANCAASw/VbAXHGTIuqCMysLtvnlfwqu+ZFr7EQiwzWmLuynjO5Z/X+o4fuRzcMX916P1"
        "LxoWt4Ywc5KEVBuTzUzjBVI")
_CRED = "LvEU0Q2YgsXFJfGYsxQS5w=="

# Everything that must never be seen leaving, in its plain spelling.
ALL = [VAR, GH, PASSWORD, ASK_PASSWORD, LOCAL_PASSWORD, COOKIE, _KEY]

SECRETS = {
    "CANARY_VAR": VAR,
    "GH_TOKEN": GH,
    "EXAMPLE": {"kind": "login", "sites": ["https://example.test"], "username": "canary-user",
                "password": PASSWORD, "totp": TOTP_SEED},
    "ASKME": {"kind": "login", "sites": ["https://example.test"], "username": "ask-user",
              "password": ASK_PASSWORD, "use": "ask"},
    "PASSKEY": {"kind": "passkey", "rpId": "example.test", "credentialId": _CRED,
                "privateKey": _KEY, "userName": "canary-user"},
    # The same throwaway key for localhost, so a page the suite serves can ask for it.
    "PASSKEYL": {"kind": "passkey", "rpId": "localhost", "credentialId": "QnJpZGdlLWNhbmFyeS1sb2NhbGhvc3Q=", "userHandle": "dGVhbW1hdGUtMQ==",
                 "privateKey": _KEY, "userName": "canary-user"},
    "LOCALAPP": {"kind": "login", "sites": ["http://localhost:8000"], "username": "local-user",
                 "password": LOCAL_PASSWORD},
}


def body():
    return json.dumps(SECRETS).encode()
