# frozen_string_literal: true
require_relative "lib/catdb"

def check(condition)
  raise "Ruby SDK smoke assertion failed" unless condition
end

auth = if ENV["CATDB_USER"]
         { username: ENV.fetch("CATDB_USER"), password: ENV.fetch("CATDB_PASSWORD"), ca_file: ENV["CATDB_CA_FILE"] }
       else
         { token: ENV.fetch("CATDB_TOKEN") }
       end
CatDB::Client.connect(host: ENV.fetch("CATDB_HOST"), port: Integer(ENV.fetch("CATDB_PORT")), **auth) do |client|
  check(client.ping.match?(/\A\d+\z/))
  next if ENV["CATDB_AUTH_ONLY"] == "1"
  key = ["ff00"].pack("H*")
  check(client.get_latest(key) == [ENV.fetch("CATDB_EXPECT")].pack("H*"))
  temporary = ["00ff72756279"].pack("H*")
  client.delete_commit(temporary)
  check(client.get_latest(temporary).nil?)
  client.begin_transaction
  client.put(temporary, "".b)
  check(client.get(temporary) == "".b)
  client.rollback
  check(client.get_latest(temporary).nil?)
  client.begin_transaction
  client.put(temporary, "".b)
  client.commit
  client.begin_read
  check(client.get(temporary) == "".b)
  client.rollback
  begin
    client.commit
    raise "catdb accepted commit without a transaction"
  rescue CatDB::ServerError => e
    check(e.code == "invalid_request")
  end
  client.delete_commit(temporary)
  client.put_commit(key, [ENV.fetch("CATDB_VALUE")].pack("H*"))
end
