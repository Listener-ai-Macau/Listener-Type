import json
import sqlite3
import time
from contextlib import contextmanager
from uuid import uuid4

from .models import Principal, ServiceError


class Store:
    """Persistent single-node store. Transactions also serialize quota reservations."""

    def __init__(self, path: str):
        self.db = sqlite3.connect(path, isolation_level=None, check_same_thread=False)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.executescript("""
        CREATE TABLE IF NOT EXISTS enrollments(
          id TEXT PRIMARY KEY, customer TEXT NOT NULL, request_id TEXT NOT NULL,
          mode TEXT NOT NULL, fingerprint TEXT NOT NULL, data TEXT NOT NULL,
          UNIQUE(customer, request_id));
        CREATE TABLE IF NOT EXISTS bindings(
          customer TEXT NOT NULL, mode TEXT NOT NULL, fingerprint TEXT NOT NULL,
          profile TEXT NOT NULL, PRIMARY KEY(customer, mode, fingerprint));
        CREATE TABLE IF NOT EXISTS sessions(
          id TEXT PRIMARY KEY, customer TEXT NOT NULL, request_id TEXT NOT NULL,
          period TEXT NOT NULL, reserved INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0,
          expires REAL NOT NULL, state TEXT NOT NULL, profile TEXT NOT NULL,
          UNIQUE(customer, request_id));
        CREATE TABLE IF NOT EXISTS usage(
          customer TEXT NOT NULL, period TEXT NOT NULL, used INTEGER NOT NULL DEFAULT 0,
          reserved INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(customer, period));
        """)
        # A process crash cannot establish whether a provider create was executed.
        for row in self.db.execute("SELECT id,data FROM enrollments").fetchall():
            data = json.loads(row[1])
            if data["state"] == "registering":
                data["state"] = "unknown"
                self.save_enrollment(row[0], data)

    @contextmanager
    def transaction(self):
        self.db.execute("BEGIN IMMEDIATE")
        try:
            yield
            self.db.execute("COMMIT")
        except BaseException:
            self.db.execute("ROLLBACK")
            raise

    def enrollment(self, customer: str, enrollment_id: str) -> dict:
        row = self.db.execute("SELECT customer,data FROM enrollments WHERE id=?",
                              (enrollment_id,)).fetchone()
        if row is None or row[0] != customer:
            raise ServiceError(404, "enrollment_not_found")
        return json.loads(row[1])

    def create_enrollment(self, customer: str, request_id: str, mode: str, fp: str) -> dict:
        with self.transaction():
            row = self.db.execute("SELECT data FROM enrollments WHERE customer=? AND request_id=?",
                                  (customer, request_id)).fetchone()
            if row:
                data = json.loads(row[0])
                if data["mode"] != mode or data["fingerprint"] != fp:
                    raise ServiceError(409, "request_id_conflict")
                return data
            for row in self.db.execute("SELECT data FROM enrollments WHERE customer=?",
                                       (customer,)).fetchall():
                active = json.loads(row[0])
                if active["state"] in {"collecting", "registering", "awaitingProvider", "unknown"}:
                    raise ServiceError(409, "enrollment_in_progress")
            enrollment_id = str(uuid4())
            data = {"id": enrollment_id, "mode": mode, "fingerprint": fp,
                    "state": "collecting", "samples": {}, "created": time.time(),
                    "speakerName": "listener_owner_" + enrollment_id.replace("-", ""),
                    "audioKey": None, "profile": None}
            self.db.execute("INSERT INTO enrollments VALUES(?,?,?,?,?,?)",
                            (enrollment_id, customer, request_id, mode, fp, json.dumps(data)))
            return data

    def save_enrollment(self, enrollment_id: str, data: dict):
        self.db.execute("UPDATE enrollments SET data=? WHERE id=?", (json.dumps(data), enrollment_id))

    def publish(self, customer: str, data: dict, profile: dict):
        with self.transaction():
            self.db.execute("INSERT OR REPLACE INTO bindings VALUES(?,?,?,?)",
                            (customer, data["mode"], data["fingerprint"], json.dumps(profile)))
            data["profile"], data["state"] = profile, "ready"
            self.save_enrollment(data["id"], data)

    def binding(self, customer: str, mode: str, fp: str) -> dict:
        row = self.db.execute("SELECT profile FROM bindings WHERE customer=? AND mode=? AND fingerprint=?",
                              (customer, mode, fp)).fetchone()
        if not row:
            raise ServiceError(409, "voiceprint_required")
        return json.loads(row[0])

    def remove_binding(self, customer: str, mode: str, fp: str):
        self.db.execute("DELETE FROM bindings WHERE customer=? AND mode=? AND fingerprint=?",
                        (customer, mode, fp))

    def reserve(self, principal: Principal, request_id: str, profile: dict, now: float) -> dict:
        if not principal.managed_enabled:
            raise ServiceError(403, "subscription_required")
        with self.transaction():
            # Expired unused tickets release reservations; running sessions have a hard deadline.
            expired = self.db.execute("SELECT id,customer,period,reserved,used FROM sessions "
                                      "WHERE state IN ('reserved','running') AND expires<?", (now,)).fetchall()
            for sid, customer, period, reserved, used in expired:
                self._finish(sid, customer, period, reserved, used)
            row = self.db.execute("SELECT * FROM sessions WHERE customer=? AND request_id=?",
                                  (principal.customer, request_id)).fetchone()
            if row:
                session = self._session(row)
                if session["state"] != "reserved" or session["period"] != principal.period:
                    raise ServiceError(409, "session_already_used")
                return session
            self.db.execute("INSERT OR IGNORE INTO usage(customer,period) VALUES(?,?)",
                            (principal.customer, principal.period))
            used, reserved = self.db.execute("SELECT used,reserved FROM usage WHERE customer=? AND period=?",
                                             (principal.customer, principal.period)).fetchone()
            available = principal.included_seconds - used - reserved
            allowance = min(180, available)
            if allowance < 3:
                raise ServiceError(402, "quota_exhausted")
            sid = str(uuid4())
            self.db.execute("UPDATE usage SET reserved=reserved+? WHERE customer=? AND period=?",
                            (allowance, principal.customer, principal.period))
            self.db.execute("INSERT INTO sessions VALUES(?,?,?,?,?,0,?,'reserved',?)",
                            (sid, principal.customer, request_id, principal.period, allowance,
                             now + 60, json.dumps(profile)))
            return self.session(principal.customer, sid)

    @staticmethod
    def _session(row) -> dict:
        return dict(zip(("id", "customer", "requestId", "period", "reserved", "used", "expires",
                         "state", "profile"), (*row[:8], json.loads(row[8]))))

    def session(self, customer: str, session_id: str) -> dict:
        row = self.db.execute("SELECT * FROM sessions WHERE id=? AND customer=?",
                              (session_id, customer)).fetchone()
        if not row:
            raise ServiceError(404, "session_not_found")
        return self._session(row)

    def consume(self, principal: Principal, sid: str, now: float) -> dict:
        with self.transaction():
            session = self.session(principal.customer, sid)
            if (not principal.managed_enabled or session["period"] != principal.period
                    or session["state"] != "reserved" or session["expires"] < now):
                raise ServiceError(403, "invalid_session_ticket")
            self.db.execute("UPDATE sessions SET state='running',expires=? WHERE id=?",
                            (now + session["reserved"] + 20, sid))
            return session

    def record_audio(self, sid: str, seconds: int):
        self.db.execute("UPDATE sessions SET used=? WHERE id=? AND state='running' AND used<?",
                        (seconds, sid, seconds))

    def _finish(self, sid: str, customer: str, period: str, reserved: int, used: int):
        self.db.execute("UPDATE usage SET reserved=MAX(0,reserved-?),used=used+? "
                        "WHERE customer=? AND period=?", (reserved, min(used, reserved), customer, period))
        self.db.execute("UPDATE sessions SET state='finished' WHERE id=?", (sid,))

    def finish(self, customer: str, sid: str):
        with self.transaction():
            s = self.session(customer, sid)
            if s["state"] in {"reserved", "running"}:
                self._finish(sid, customer, s["period"], s["reserved"], s["used"])
