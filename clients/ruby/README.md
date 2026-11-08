# catdb for Ruby

Use Ruby 3.1 or later. Install the client from RubyGems.org:

```sh
gem install catdb-client -v 0.1.0
```

With Bundler, add `gem "catdb-client", "~> 0.1.0"` to your Gemfile and run `bundle install`. Load the client with `require "catdb"`.

```ruby
require "catdb"

CatDB::Client.connect(token: ENV.fetch("CATDB_TOKEN")) do |client|
  key = "\xff\x00".b
  version = client.put_commit(key, "\x01\x02".b)
  value = client.get_latest(key)
  p [version, value]
end
```

Use the shared token on a trusted local connection. For an account, pass `host:`, `username:`, `password:` and optionally `database:` (default `"main"`) and `ca_file:` (a PEM certificate file). Account connections require TLS 1.3 and verify the hostname and certificate chain. The default address is `127.0.0.1:7400`. The `timeout:` setting is in seconds and defaults to 10.

`get_latest` returns a binary string or `nil` for a missing key. An empty value returns an empty string. `put_commit` and `delete_commit` wait for a durable commit and return its version as a decimal string. For a transaction, use `begin_transaction` or `begin_read`, then `get`, `put`, `delete`, `commit` or `rollback`. One connection serializes requests and owns one transaction. Give independent transactions separate clients. The block form closes the connection on exit.

Use `request(op: "scan_latest", start: "", rows: 20)` for the remaining wire operations, with hex keys and values. A server rejection raises `CatDB::ServerError` with its `code`. A lost reply can leave a mutation's outcome unknown. The client closes the connection and leaves the decision to retry to the application.
