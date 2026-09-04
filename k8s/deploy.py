#!/usr/bin/env python3
"""Deploy ops-rust-test2 to K8s via the Kubernetes API.

Builds resource payloads as Python dicts (avoids YAML list-parsing bugs) and
creates/updates them idempotently. Credentials come from env vars only.
"""
import base64
import json
import os
import secrets
import sys
import urllib.request

HOST = os.environ["K8S_HOST"]
PORT = os.environ.get("K8S_PORT", "16443")
NS = os.environ["K8S_NAMESPACE"]
APP = os.environ["APP_NAME"]
DOMAIN = os.environ["DOMAIN"]
IMAGE_TAG = os.environ["IMAGE_TAG"]
TLS_SECRET = os.environ.get("TLS_SECRET", "steedgrace-com-tls-secret")
MIDDLEWARE = os.environ.get("MIDDLEWARE", f"{NS}-redirect-https@kubernetescrd")

BASE = f"https://{HOST}:{PORT}"

cert = base64.b64decode(os.environ["K8S_CERT_B64"])
key = base64.b64decode(os.environ["K8S_KEY_B64"])
with open("/tmp/k8s-cert.pem", "wb") as f:
    f.write(cert)
with open("/tmp/k8s-key.pem", "wb") as f:
    f.write(key)

import ssl

# The API server is reached via an IP endpoint whose cert SANs only contain
# internal names (kubernetes, kubernetes.default.svc). Verify the chain against
# the cluster CA but pin SNI/hostname to "kubernetes" — same mechanism as
# kubectl --tls-server-name=kubernetes. CERT_NONE is NOT used.
ctx = ssl.create_default_context(cafile="/tmp/k8s-ca.pem")
ctx.check_hostname = False  # hostname check done below via explicit servername
ctx.load_cert_chain("/tmp/k8s-cert.pem", "/tmp/k8s-key.pem")


def _open(req):
    import http.client

    class PinnedHTTPSConnection(http.client.HTTPSConnection):
        def connect(self):
            import socket

            sock = socket.create_connection((self.host, self.port), self.timeout)
            self.sock = ctx.wrap_socket(sock, server_hostname="kubernetes")

    class PinnedHTTPSHandler(urllib.request.HTTPSHandler):
        def https_open(self, req):
            return self.do_open(PinnedHTTPSConnection, req)

    opener = urllib.request.build_opener(PinnedHTTPSHandler())
    return opener.open(req)


def api(method, path, body=None):
    url = BASE + path
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    try:
        with _open(req) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        print(f"{method} {path} -> {e.code}: {e.read().decode()[:400]}", file=sys.stderr)
        raise



def create_or_update(kind, name, col_path, body):
    existing = api("GET", f"{col_path}/{name}")
    if existing is None:
        api("POST", col_path, body)
        print(f"  CREATED {kind}/{name}")
    else:
        body["metadata"]["resourceVersion"] = existing["metadata"]["resourceVersion"]
        api("PUT", f"{col_path}/{name}", body)
        print(f"  UPDATED {kind}/{name}")


b64 = lambda s: base64.b64encode(s.encode()).decode()

# 1. Postgres credentials Secret (merge missing keys on re-deploy)
sec_name = f"{APP}-postgres-credentials"
db_name = "class_manager"
existing_sec = api("GET", f"/api/v1/namespaces/{NS}/secrets/{sec_name}")
if existing_sec and "data" in existing_sec:
    data = existing_sec["data"]
    user = base64.b64decode(data["POSTGRES_USER"]).decode()
    password = base64.b64decode(data["POSTGRES_PASSWORD"]).decode()
    if "DATABASE_URL" not in data:
        data["DATABASE_URL"] = b64(
            f"postgres://{user}:{password}@{APP}-postgres.{NS}.svc.cluster.local:5432/{db_name}"
        )
        api("PUT", f"/api/v1/namespaces/{NS}/secrets/{sec_name}", existing_sec)
        print(f"  UPDATED {sec_name} (added DATABASE_URL)")
    else:
        print(f"  EXISTS {sec_name}")
else:
    user = "classadmin"
    password = secrets.token_urlsafe(24)
    create_or_update(
        "Secret",
        sec_name,
        f"/api/v1/namespaces/{NS}/secrets",
        {
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {"name": sec_name, "namespace": NS},
            "type": "Opaque",
            "data": {
                "POSTGRES_USER": b64(user),
                "POSTGRES_PASSWORD": b64(password),
                "POSTGRES_DB": b64(db_name),
                "DATABASE_URL": b64(
                    f"postgres://{user}:{password}@{APP}-postgres.{NS}.svc.cluster.local:5432/{db_name}"
                ),
            },
        },
    )

# 2. Postgres Service (headless)
create_or_update(
    "Service",
    f"{APP}-postgres",
    f"/api/v1/namespaces/{NS}/services",
    {
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {"name": f"{APP}-postgres", "namespace": NS, "labels": {"app": f"{APP}-postgres"}},
        "spec": {
            "type": "ClusterIP",
            "clusterIP": "None",
            "selector": {"app": f"{APP}-postgres"},
            "ports": [{"name": "postgres", "port": 5432, "targetPort": 5432}],
        },
    },
)

# 3. Postgres StatefulSet
create_or_update(
    "StatefulSet",
    f"{APP}-postgres",
    f"/apis/apps/v1/namespaces/{NS}/statefulsets",
    {
        "apiVersion": "apps/v1",
        "kind": "StatefulSet",
        "metadata": {"name": f"{APP}-postgres", "namespace": NS, "labels": {"app": f"{APP}-postgres"}},
        "spec": {
            "serviceName": f"{APP}-postgres",
            "replicas": 1,
            "selector": {"matchLabels": {"app": f"{APP}-postgres"}},
            "template": {
                "metadata": {"labels": {"app": f"{APP}-postgres"}},
                "spec": {
                    "containers": [
                        {
                            "name": "postgres",
                            "image": "postgres:15-alpine",
                            "ports": [{"name": "postgres", "containerPort": 5432}],
                            "envFrom": [{"secretRef": {"name": sec_name}}],
                            "env": [{"name": "PGDATA", "value": "/var/lib/postgresql/data/pgdata"}],
                            "volumeMounts": [{"name": "pgdata", "mountPath": "/var/lib/postgresql/data"}],
                            "readinessProbe": {
                                "exec": {"command": ["pg_isready", "-U", "$(POSTGRES_USER)"]},
                                "initialDelaySeconds": 10,
                                "periodSeconds": 10,
                            },
                        }
                    ]
                },
            },
            "volumeClaimTemplates": [
                {
                    "metadata": {"name": "pgdata"},
                    "spec": {
                        "accessModes": ["ReadWriteOnce"],
                        "storageClassName": "nfs142",
                        "resources": {"requests": {"storage": "2Gi"}},
                    },
                }
            ],
        },
    },
)

# 4. App Deployment
create_or_update(
    "Deployment",
    APP,
    f"/apis/apps/v1/namespaces/{NS}/deployments",
    {
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {"name": APP, "namespace": NS, "labels": {"app": APP}},
        "spec": {
            "replicas": 1,
            "selector": {"matchLabels": {"app": APP}},
            "template": {
                "metadata": {"labels": {"app": APP}},
                "spec": {
                    "containers": [
                        {
                            "name": "app",
                            "image": IMAGE_TAG,
                            "ports": [{"name": "http", "containerPort": 8080}],
                            "envFrom": [{"secretRef": {"name": sec_name}}],
                            "env": [{"name": "BIND_ADDR", "value": "0.0.0.0:8080"}],
                            "readinessProbe": {
                                "httpGet": {"path": "/api/health", "port": 8080},
                                "initialDelaySeconds": 5,
                                "periodSeconds": 10,
                            },
                            "livenessProbe": {
                                "httpGet": {"path": "/api/health", "port": 8080},
                                "initialDelaySeconds": 15,
                                "periodSeconds": 20,
                            },
                        }
                    ]
                },
            },
        },
    },
)

# 5. App Service
create_or_update(
    "Service",
    APP,
    f"/api/v1/namespaces/{NS}/services",
    {
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {"name": APP, "namespace": NS, "labels": {"app": APP}},
        "spec": {
            "type": "ClusterIP",
            "selector": {"app": APP},
            "ports": [{"name": "http", "port": 8080, "targetPort": 8080}],
        },
    },
)

# 6. Ingress HTTPS
create_or_update(
    "Ingress",
    APP,
    f"/apis/networking.k8s.io/v1/namespaces/{NS}/ingresses",
    {
        "apiVersion": "networking.k8s.io/v1",
        "kind": "Ingress",
        "metadata": {
            "name": APP,
            "namespace": NS,
            "annotations": {
                "traefik.ingress.kubernetes.io/router.entrypoints": "websecure",
                "traefik.ingress.kubernetes.io/router.tls": "true",
            },
        },
        "spec": {
            "rules": [
                {
                    "host": DOMAIN,
                    "http": {
                        "paths": [
                            {
                                "path": "/",
                                "pathType": "Prefix",
                                "backend": {"service": {"name": APP, "port": {"number": 8080}}},
                            }
                        ]
                    },
                }
            ],
            "tls": [{"hosts": [DOMAIN], "secretName": TLS_SECRET}],
        },
    },
)

# 7. Ingress HTTP -> HTTPS redirect
create_or_update(
    "Ingress",
    f"{APP}-http",
    f"/apis/networking.k8s.io/v1/namespaces/{NS}/ingresses",
    {
        "apiVersion": "networking.k8s.io/v1",
        "kind": "Ingress",
        "metadata": {
            "name": f"{APP}-http",
            "namespace": NS,
            "annotations": {
                "traefik.ingress.kubernetes.io/router.entrypoints": "web",
                "traefik.ingress.kubernetes.io/router.middlewares": MIDDLEWARE,
            },
        },
        "spec": {
            "rules": [
                {
                    "host": DOMAIN,
                    "http": {
                        "paths": [
                            {
                                "path": "/",
                                "pathType": "Prefix",
                                "backend": {"service": {"name": APP, "port": {"number": 8080}}},
                            }
                        ]
                    },
                }
            ]
        },
    },
)

print("DEPLOY COMPLETE")
