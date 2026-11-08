package catdb

import (
	"encoding/binary"
	"errors"
	"io"
	"net"
	"strings"
	"testing"
	"time"
)

func TestMalformedTypedReplyClosesConnection(t *testing.T) {
	for _, test := range []struct {
		name, payload string
		write         bool
	}{
		{"missing value", `{"ok":true}`, false},
		{"wrong value type", `{"ok":true,"value":42}`, false},
		{"malformed hex", `{"ok":true,"value":"zz"}`, false},
		{"missing committed version", `{"ok":true}`, true},
		{"numeric version", `{"ok":true,"version":3}`, true},
		{"version overflow", `{"ok":true,"version":"18446744073709551616"}`, true},
	} {
		t.Run(test.name, func(t *testing.T) {
			conn, server := net.Pipe()
			client := &Client{conn: conn, timeout: time.Second}
			defer client.Close()
			defer server.Close()
			done := make(chan error, 1)
			go func() {
				var header [4]byte
				_, err := io.ReadFull(server, header[:])
				if err == nil {
					_, err = io.CopyN(io.Discard, server, int64(binary.BigEndian.Uint32(header[:])))
				}
				if err == nil {
					binary.BigEndian.PutUint32(header[:], uint32(len(test.payload)))
					_, err = server.Write(append(header[:], test.payload...))
				}
				done <- err
			}()
			var err error
			if test.write {
				_, err = client.PutCommit([]byte("k"), []byte("v"))
			} else {
				_, err = client.GetLatest([]byte("k"))
			}
			if err == nil || !strings.Contains(err.Error(), "may have committed") {
				t.Fatalf("unexpected exchange error: %v", err)
			}
			if err := <-done; err != nil {
				t.Fatal(err)
			}
			if _, err := client.Ping(); !errors.Is(err, net.ErrClosed) {
				t.Fatalf("connection still usable: %v", err)
			}
		})
	}
}
