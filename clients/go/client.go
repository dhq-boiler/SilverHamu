// Package silverhamu connects to the silverhamu TCP service.
package silverhamu

import (
	"crypto/tls"
	"crypto/x509"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"strconv"
	"sync"
	"time"
	"unicode/utf8"
)

const (
	maxRequest  = 1024 * 1024
	maxResponse = 2*1024*1024 + 8192
)

// ServerError carries the error code returned by silverhamu.
type ServerError struct{ Code string }

func (e *ServerError) Error() string { return "silverhamu: " + e.Code }

// Client serializes wire requests. Give each concurrent transaction its own Client.
type Client struct {
	mu      sync.Mutex
	conn    net.Conn
	timeout time.Duration
}

// DialToken connects to the plaintext shared-token listener. Address is host:port.
func DialToken(address, token string, timeout time.Duration) (*Client, error) {
	return dial(address, nil, map[string]any{"op": "auth", "version": 1, "token": token}, 1, timeout)
}

// DialAccount connects using verified TLS 1.3. Nil caPEM selects the system roots.
func DialAccount(address, username, password, database string, caPEM []byte, timeout time.Duration) (*Client, error) {
	host, _, err := net.SplitHostPort(address)
	if err != nil {
		return nil, err
	}
	config := &tls.Config{MinVersion: tls.VersionTLS13, ServerName: host}
	if caPEM != nil {
		config.RootCAs = x509.NewCertPool()
		if !config.RootCAs.AppendCertsFromPEM(caPEM) {
			return nil, errors.New("invalid CA certificate")
		}
	}
	return dial(address, config, map[string]any{"op": "auth", "version": 2, "username": username, "password": password, "database": database}, 2, timeout)
}

func dial(address string, config *tls.Config, auth map[string]any, protocol int, timeout time.Duration) (*Client, error) {
	if timeout <= 0 {
		return nil, errors.New("timeout must be positive")
	}
	d := net.Dialer{Timeout: timeout}
	var conn net.Conn
	var err error
	if config != nil {
		conn, err = tls.DialWithDialer(&d, "tcp", address, config)
	} else {
		conn, err = d.Dial("tcp", address)
	}
	if err != nil {
		return nil, err
	}
	c := &Client{conn: conn, timeout: timeout}
	reply, err := c.Request(auth)
	if err != nil {
		c.Close()
		return nil, err
	}
	var version int
	if json.Unmarshal(reply["protocol"], &version) != nil || version != protocol {
		c.Close()
		return nil, errors.New("unexpected silverhamu protocol version")
	}
	return c, nil
}

// Close releases the socket. The server discards any staged transaction on disconnect.
func (c *Client) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.conn == nil {
		return nil
	}
	err := c.conn.Close()
	c.conn = nil
	return err
}

// Request exchanges a single protocol message, including operations such as scan.
// A transport failure closes the connection. A submitted write may already be committed.
func (c *Client) Request(request map[string]any) (map[string]json.RawMessage, error) {
	return execute(c, request, func(reply map[string]json.RawMessage) (map[string]json.RawMessage, error) {
		return reply, nil
	})
}

func execute[T any](c *Client, request map[string]any, decode func(map[string]json.RawMessage) (T, error)) (T, error) {
	var zero T
	payload, err := json.Marshal(request)
	if err != nil {
		return zero, err
	}
	if len(payload) > maxRequest {
		return zero, errors.New("silverhamu request exceeds 1 MiB")
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.conn == nil {
		return zero, net.ErrClosed
	}
	reply, err := c.exchange(payload)
	if err == nil {
		var result T
		result, err = decode(reply)
		if err == nil {
			return result, nil
		}
	}
	var rejected *ServerError
	if errors.As(err, &rejected) {
		return zero, err
	}
	c.conn.Close()
	c.conn = nil
	return zero, fmt.Errorf("silverhamu exchange failed; a submitted write may have committed: %w", err)
}

func (c *Client) exchange(payload []byte) (map[string]json.RawMessage, error) {
	if err := c.conn.SetDeadline(time.Now().Add(c.timeout)); err != nil {
		return nil, err
	}
	frame := make([]byte, 4+len(payload))
	binary.BigEndian.PutUint32(frame, uint32(len(payload)))
	copy(frame[4:], payload)
	n, err := c.conn.Write(frame)
	if err != nil {
		return nil, err
	}
	if n != len(frame) {
		return nil, io.ErrShortWrite
	}
	var header [4]byte
	if _, err := io.ReadFull(c.conn, header[:]); err != nil {
		return nil, err
	}
	size := binary.BigEndian.Uint32(header[:])
	if size == 0 || size > maxResponse {
		return nil, errors.New("invalid silverhamu response size")
	}
	body := make([]byte, size)
	if _, err := io.ReadFull(c.conn, body); err != nil {
		return nil, err
	}
	if !utf8.Valid(body) {
		return nil, errors.New("invalid response encoding")
	}
	var reply map[string]json.RawMessage
	if err := json.Unmarshal(body, &reply); err != nil {
		return nil, err
	}
	switch string(reply["ok"]) {
	case "true":
		return reply, nil
	case "false":
		var code string
		if json.Unmarshal(reply["error"], &code) != nil || code == "" {
			return nil, errors.New("invalid silverhamu error response")
		}
		return nil, &ServerError{Code: code}
	default:
		return nil, errors.New("missing silverhamu response status")
	}
}

func (c *Client) version(request map[string]any) (uint64, error) {
	return execute(c, request, decodeVersion)
}

func decodeVersion(reply map[string]json.RawMessage) (uint64, error) {
	var version string
	if err := json.Unmarshal(reply["version"], &version); err != nil {
		return 0, err
	}
	return strconv.ParseUint(version, 10, 64)
}

func (c *Client) read(op string, key []byte) ([]byte, error) {
	return execute(c, map[string]any{"op": op, "key": hex.EncodeToString(key)}, decodeValue)
}

func decodeValue(reply map[string]json.RawMessage) ([]byte, error) {
	if string(reply["value"]) == "null" {
		return nil, nil
	}
	var value string
	if err := json.Unmarshal(reply["value"], &value); err != nil {
		return nil, err
	}
	return hex.DecodeString(value)
}

// GetLatest returns nil for a missing key and a non-nil zero-length slice for a stored empty value.
func (c *Client) GetLatest(key []byte) ([]byte, error) { return c.read("get_latest", key) }
func (c *Client) PutCommit(key, value []byte) (uint64, error) {
	return c.version(map[string]any{"op": "put_commit", "key": hex.EncodeToString(key), "value": hex.EncodeToString(value)})
}
func (c *Client) DeleteCommit(key []byte) (uint64, error) {
	return c.version(map[string]any{"op": "delete_commit", "key": hex.EncodeToString(key)})
}
func (c *Client) Ping() (uint64, error)          { return c.version(map[string]any{"op": "ping"}) }
func (c *Client) Begin() (uint64, error)         { return c.version(map[string]any{"op": "begin"}) }
func (c *Client) BeginRead() (uint64, error)     { return c.version(map[string]any{"op": "begin_read"}) }
func (c *Client) Get(key []byte) ([]byte, error) { return c.read("get", key) }
func (c *Client) Put(key, value []byte) error {
	_, err := c.Request(map[string]any{"op": "put", "key": hex.EncodeToString(key), "value": hex.EncodeToString(value)})
	return err
}
func (c *Client) Delete(key []byte) error {
	_, err := c.Request(map[string]any{"op": "delete", "key": hex.EncodeToString(key)})
	return err
}
func (c *Client) Commit() (uint64, error) { return c.version(map[string]any{"op": "commit"}) }
func (c *Client) Rollback() error         { _, err := c.Request(map[string]any{"op": "rollback"}); return err }
