'use strict'

var assert = require('node:assert/strict')
var express = require('..')
var request = require('supertest')

describe('query parser depth contract', function () {
  it('limits bracket nesting in values read by a route', function (done) {
    var app = express()
    app.set('query parser', 'extended')
    app.set('query parser depth', 1)
    app.get('/', function (req, res) { res.json(req.query) })

    request(app)
      .get('/?account[profile][name]=Ada')
      .expect(200, { account: { profile: { '[name]': 'Ada' } } }, done)
  })

  it('retains the default and rejects invalid depths', function () {
    var app = express()
    assert.strictEqual(app.get('query parser depth'), 5)
    assert.throws(function () { app.set('query parser depth', -1) }, /non-negative integer/)
    assert.throws(function () { app.set('query parser depth', 1.5) }, /non-negative integer/)
  })

  it('leaves bracketed keys flat at depth zero', function (done) {
    var app = express()
    app.set('query parser', 'extended')
    app.set('query parser depth', 0)
    app.get('/', function (req, res) { res.json(req.query) })

    request(app)
      .get('/?account[profile][name]=Ada')
      .expect(200, { 'account[profile][name]': 'Ada' }, done)
  })

  it('uses a changed depth on later route requests', function (done) {
    var app = express()
    app.set('query parser', 'extended')
    app.set('query parser depth', 2)
    app.get('/', function (req, res) { res.json(req.query) })

    request(app)
      .get('/?account[profile][name]=Ada')
      .expect(200, { account: { profile: { name: 'Ada' } } }, function (error) {
        if (error) return done(error)

        app.set('query parser depth', 1)

        request(app)
          .get('/?account[profile][name]=Ada')
          .expect(200, { account: { profile: { '[name]': 'Ada' } } }, done)
      })
  })

  it('keeps simple parsing independent of depth', function (done) {
    var app = express()
    app.set('query parser', 'simple')
    app.set('query parser depth', 0)
    app.get('/', function (req, res) { res.json(req.query) })

    request(app)
      .get('/?user%5Bname%5D=tj')
      .expect(200, { 'user[name]': 'tj' }, done)
  })

  it('keeps disabled parsing independent of depth', function (done) {
    var app = express()
    app.set('query parser', false)
    app.set('query parser depth', 0)
    app.get('/', function (req, res) { res.json(req.query) })

    request(app)
      .get('/?user%5Bname%5D=tj')
      .expect(200, {}, done)
  })

  it('keeps a custom parser independent of depth', function (done) {
    var app = express()
    app.set('query parser', function (str) { return { raw: str } })
    app.set('query parser depth', 0)
    app.get('/', function (req, res) { res.json(req.query) })

    request(app)
      .get('/?user%5Bname%5D=tj')
      .expect(200, { raw: 'user%5Bname%5D=tj' }, done)
  })
})
