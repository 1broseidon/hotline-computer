"""The made-up secrets this suite delivers. Nothing here is real: every value
is a canary invented for the probes, and the passkey is a throwaway P-256 key."""

import json

VAR = "hl-canary-7f3a"                      # CANARY_VAR, a plain variable
GH = "ghp_hlCanaryW5Token0000000000000000000"  # GH_TOKEN, a key-shaped variable
PASSWORD = "hl-canary-pw-91c2"              # EXAMPLE.password
ASK_PASSWORD = "hl-canary-ask-55e0"         # ASKME.password
LOCAL_PASSWORD = "hl-canary-local-3b8d"     # LOCALAPP.password, on http://localhost:8000
# LOCALENC.password, on http://localhost:8000: every character a URL, a form
# or HTML spells another way (space, / + & = % < ").
LOCAL_ENC = 'hl canary/enc+5&k=9%<"x'
# LOOPAPP.password, on http://localhost:8002, delivered v2 without readable:
# a loopback site, so the computer must not type it (finding 4).
LOOP_PASSWORD = "hl-canary-loop-6d2e"
COOKIE = "hl-canary-cookie-w5-8c41"         # a signed-in session cookie on example.test
TOTP_SEED = "JBSWY3DPEHPK3PXPJBSWY3DP"

_KEY = ("MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgB9aEvdD1k7n55Wu+sDtf5cUbUumdw/"
        "XyKmgddnpO1eihRANCAASw/VbAXHGTIuqCMysLtvnlfwqu+ZFr7EQiwzWmLuynjO5Z/X+o4fuRzcMX916P1"
        "LxoWt4Ywc5KEVBuTzUzjBVI")
_CRED = "LvEU0Q2YgsXFJfGYsxQS5w=="

# Everything that must never be seen leaving, in its plain spelling.
ALL = [VAR, GH, PASSWORD, ASK_PASSWORD, LOCAL_PASSWORD, LOCAL_ENC, LOOP_PASSWORD, COOKIE, _KEY]

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
    "LOCALENC": {"kind": "login", "sites": ["http://localhost:8000"], "username": "enc-user",
                 "password": LOCAL_ENC},
}

# Only in a v2 delivery, and without readable: a login on a site any job can
# serve. LOCALAPP and LOCALENC go v2 with readable: true (granted knowing a
# job's page can read them), so the probes that type them still run there.
LOOPBACK = {"LOOPAPP": {"kind": "login", "sites": ["http://localhost:8002"], "username": "loop-user",
                        "password": LOOP_PASSWORD}}
READABLE_LOGINS = ("LOCALAPP", "LOCALENC")


V2 = "application/vnd.hotline.secrets.v2+json"
# The variables a v2 run delivers readable (today's behaviour, labelled) or
# protected (bound to the fixture's origin), and the stand-in shape asked for.
VARIABLES = {"CANARY_VAR": None, "GH_TOKEN": {"prefix": "ghp_", "length": 40}}


def body(delivery="v1", origin=None):
    """(bytes, content type). v1 is the body a desk before 0.50 sends. v2
    "readable" marks the variables readable:true; v2 "protected" binds them
    to `origin`, the fixture, so a job holds only a stand-in."""
    if delivery == "v1":
        return json.dumps(SECRETS).encode(), "application/json"
    secrets = {}
    for name, entry in {**SECRETS, **LOOPBACK}.items():
        if name in READABLE_LOGINS:
            secrets[name] = dict(entry, readable=True)
        elif name not in VARIABLES:
            secrets[name] = entry
        elif delivery == "readable":
            secrets[name] = {"kind": "variable", "value": entry, "readable": True}
        else:
            use = {"origins": [origin], "header": "Authorization", "schemes": ["Bearer", "token", "Basic"]}
            if VARIABLES[name]:
                use["standIn"] = VARIABLES[name]
            secrets[name] = {"kind": "variable", "value": entry, "use": use}
    return json.dumps({"v": 2, "generation": 1, "secrets": secrets}).encode(), V2
