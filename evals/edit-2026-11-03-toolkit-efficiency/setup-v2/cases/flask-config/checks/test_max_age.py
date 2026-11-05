from datetime import datetime
from datetime import timedelta
from datetime import timezone
from email.utils import parsedate_to_datetime
from http.cookies import SimpleCookie

from flask import Flask
from flask import session


def test_max_age_setting_controls_permanent_set_cookie_expiry():
    app = Flask(__name__)
    app.secret_key = "test"
    app.config["PERMANENT_SESSION_LIFETIME"] = timedelta(days=10)
    app.config["SESSION_COOKIE_MAX_AGE"] = timedelta(seconds=60)

    @app.get("/")
    def issue_cookie():
        session["value"] = "set"
        session.permanent = True
        return ""

    before = datetime.now(timezone.utc)
    response = app.test_client().get("/")
    cookie = SimpleCookie(response.headers["Set-Cookie"])["session"]
    expires = parsedate_to_datetime(cookie["expires"])

    assert timedelta(seconds=45) < expires - before <= timedelta(seconds=60), "SESSION_COOKIE_MAX_AGE is ignored"
