"""TEST ONLY: the HTTPS fake upstream a protected key is bound to in the probe
run. It listens on the host's container-bridge address, as the plain listener
does, with a certificate for that address from a CA made for this run alone.

The key proxy trusts upstream only the web roots compiled into it. A probe
image built with `--features probe-roots` (never a release) also trusts the
PEM in HOTLINE_KEYPROXY_PROBE_ROOT, which the harness sets to this CA when it
creates the container. Certificates are made with the openssl command line;
keys stay in a temporary folder the harness removes."""
import os
import shutil
import subprocess
import tempfile


class FixtureCA:
    def __init__(self):
        self.dir = tempfile.mkdtemp(prefix="hl-secrets-probe-ca-")
        self.key = os.path.join(self.dir, "ca.key")
        self.pem_path = os.path.join(self.dir, "ca.pem")
        self._openssl("req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
                      "-keyout", self.key, "-out", self.pem_path, "-days", "2",
                      "-subj", "/CN=hl-secrets probe fixture CA (test only)",
                      "-addext", "basicConstraints=critical,CA:TRUE",
                      "-addext", "keyUsage=critical,keyCertSign,cRLSign")
        with open(self.pem_path) as f:
            self.pem = f.read()

    def _openssl(self, *args):
        r = subprocess.run(["openssl", *args], capture_output=True, text=True)
        if r.returncode != 0:
            raise RuntimeError("openssl " + args[0] + ": " + r.stderr.strip()[-300:])

    def leaf(self, ip):
        """A server certificate for `ip`: (cert path, key path)."""
        key = os.path.join(self.dir, "leaf.key")
        csr = os.path.join(self.dir, "leaf.csr")
        cert = os.path.join(self.dir, "leaf.pem")
        ext = os.path.join(self.dir, "leaf.ext")
        with open(ext, "w") as f:
            f.write(f"subjectAltName=IP:{ip}\nbasicConstraints=critical,CA:FALSE\n"
                    "keyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n"
                    "subjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid\n")
        self._openssl("req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
                      "-keyout", key, "-out", csr, "-subj", "/CN=hl-secrets probe fixture")
        self._openssl("x509", "-req", "-in", csr, "-CA", self.pem_path, "-CAkey", self.key,
                      "-CAcreateserial", "-out", cert, "-days", "2", "-extfile", ext)
        return cert, key

    def remove(self):
        shutil.rmtree(self.dir, ignore_errors=True)
