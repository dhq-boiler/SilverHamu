# frozen_string_literal: true

require "json"
require "openssl"
require "socket"
require "timeout"

module SilverHamu
  class ServerError < StandardError
    attr_reader :code

    def initialize(code)
      @code = code
      super("silverhamu returned #{code}")
    end
  end

  class Client
    MAX_REQUEST = 1024 * 1024
    MAX_RESPONSE = 2 * 1024 * 1024 + 8192

    def self.connect(**options)
      client = new(**options)
      return client unless block_given?

      begin
        yield client
      ensure
        client.close
      end
    end

    def initialize(host: "127.0.0.1", port: 7400, token: nil, username: nil, password: nil,
                   database: "main", ca_file: nil, timeout: 10)
      @timeout = Float(timeout)
      raise ArgumentError, "timeout must be finite and positive" unless @timeout.finite? && @timeout.positive?
      account = !username.nil?
      valid = if account
                username.is_a?(String) && password.is_a?(String) && token.nil?
              else
                token.is_a?(String) && password.nil? && database == "main" && ca_file.nil?
              end
      raise ArgumentError, "choose a token or a TLS username and password" unless valid

      @lock = Mutex.new
      connected = false
      begin
        Timeout.timeout(@timeout) do
          @socket = Socket.tcp(host, port, connect_timeout: @timeout)
          @socket.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)
          if account
            context = OpenSSL::SSL::SSLContext.new
            context.set_params
            context.min_version = OpenSSL::SSL::TLS1_3_VERSION
            if ca_file
              trust = OpenSSL::X509::Store.new
              trust.add_file(ca_file)
              context.cert_store = trust
            end
            tls = OpenSSL::SSL::SSLSocket.new(@socket, context)
            tls.sync_close = true
            @socket = tls
            tls.hostname = host
            tls.connect
            tls.post_connection_check(host)
          end
        end
        auth = account ? { op: "auth", version: 2, username: username, password: password, database: database } :
                         { op: "auth", version: 1, token: token }
        reply = request(**auth)
        raise IOError, "unexpected silverhamu protocol version" unless reply["protocol"] == (account ? 2 : 1)
        connected = true
      ensure
        close unless connected
      end
    end

    def request(op:, **fields)
      body = JSON.generate(fields.merge(op: op)).b
      raise ArgumentError, "silverhamu request exceeds 1 MiB" if body.bytesize > MAX_REQUEST
      @lock.synchronize do
        socket = @socket
        raise IOError, "silverhamu connection is closed" unless socket
        received = false
        begin
          reply = Timeout.timeout(@timeout) do
            socket.write([body.bytesize].pack("N") + body)
            size = read_exact(socket, 4).unpack1("N")
            raise IOError, "invalid silverhamu response size" unless size.between?(1, MAX_RESPONSE)
            text = read_exact(socket, size).force_encoding(Encoding::UTF_8)
            raise IOError, "invalid silverhamu response encoding" unless text.valid_encoding?
            JSON.parse(text)
          end
          unless reply.is_a?(Hash) && (reply["ok"] == true || (reply["ok"] == false && reply["error"].is_a?(String)))
            raise IOError, "invalid silverhamu response"
          end
          received = true
        rescue IOError, SystemCallError, OpenSSL::SSL::SSLError, Timeout::Error, JSON::ParserError => e
          raise IOError, "silverhamu response lost or invalid; a submitted mutation may have committed", cause: e
        ensure
          close unless received
        end
        raise ServerError, reply["error"] unless reply["ok"]
        reply
      end
    end

    def ping = version(op: "ping")
    def get_latest(key) = read_value("get_latest", key)
    def put_commit(key, value) = version(op: "put_commit", key: hex(key), value: hex(value))
    def delete_commit(key) = version(op: "delete_commit", key: hex(key))
    def begin_transaction = version(op: "begin")
    def begin_read = version(op: "begin_read")
    def get(key) = read_value("get", key)
    def put(key, value) = request(op: "put", key: hex(key), value: hex(value))
    def delete(key) = request(op: "delete", key: hex(key))
    def commit = version(op: "commit")
    def rollback = request(op: "rollback")

    def close
      socket = @socket
      @socket = nil
      socket&.close
    end

    private

    def hex(bytes)
      raise TypeError, "expected a binary String" unless bytes.is_a?(String)
      bytes.unpack1("H*")
    end

    def read_exact(socket, size)
      data = socket.read(size)
      raise EOFError, "silverhamu connection closed" unless data && data.bytesize == size
      data
    end

    def read_value(op, key)
      reply = request(op: op, key: hex(key))
      value = reply["value"]
      return nil if reply.key?("value") && value.nil?
      return [value].pack("H*") if value.is_a?(String) && value.bytesize.even? && value.match?(/\A[0-9a-fA-F]*\z/)
      close
      raise IOError, "invalid silverhamu value"
    end

    def version(**operation)
      number = request(**operation)["version"]
      return number if number.is_a?(String) && number.match?(/\A(?:0|[1-9][0-9]*)\z/)
      close
      raise IOError, "invalid silverhamu version"
    end
  end
end
