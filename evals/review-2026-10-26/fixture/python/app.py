from flask import Flask, jsonify, request

app = Flask(__name__)


def build_registration_response(email):
    return {"email": email.strip().lower()}


@app.post("/register")
def register_account():
    return jsonify(build_registration_response(request.json["email"])), 201
