package main

import (
	"bytes"
	"encoding/hex"
	"errors"
	"fmt"
	"net"
	"os"
	"time"

	"github.com/dhq-boiler/catdb/clients/go"
)

func must[T any](value T, err error) T {
	if err != nil {
		panic(err)
	}
	return value
}
func check(err error) {
	if err != nil {
		panic(err)
	}
}
func require(ok bool, what string) {
	if !ok {
		panic(what)
	}
}

func main() {
	address := net.JoinHostPort(os.Getenv("CATDB_HOST"), os.Getenv("CATDB_PORT"))
	const timeout = 10 * time.Second
	var c *silverhamu.Client
	if token := os.Getenv("CATDB_TOKEN"); token != "" {
		c = must(silverhamu.DialToken(address, token, timeout))
	} else {
		var ca []byte
		if path := os.Getenv("CATDB_CA_FILE"); path != "" {
			ca = must(os.ReadFile(path))
		}
		c = must(silverhamu.DialAccount(address, os.Getenv("CATDB_USER"), os.Getenv("CATDB_PASSWORD"), "main", ca, timeout))
	}
	defer c.Close()
	must(c.Ping())
	if os.Getenv("CATDB_AUTH_ONLY") == "1" {
		return
	}
	key := []byte{0xff, 0}
	value := must(hex.DecodeString(os.Getenv("CATDB_VALUE")))
	expected := must(hex.DecodeString(os.Getenv("CATDB_EXPECT")))
	read := must(c.GetLatest(key))
	require(read != nil && bytes.Equal(read, expected), "cross-language read")
	private := []byte{0xfe, 'g', 'o'}
	must(c.DeleteCommit(private))
	require(must(c.GetLatest(private)) == nil, "missing key")
	must(c.Begin())
	check(c.Put(private, []byte{}))
	staged := must(c.Get(private))
	require(staged != nil && len(staged) == 0, "staged empty value")
	check(c.Rollback())
	require(must(c.GetLatest(private)) == nil, "rollback")
	must(c.Begin())
	check(c.Put(private, []byte{}))
	must(c.Commit())
	must(c.BeginRead())
	empty := must(c.Get(private))
	require(empty != nil && len(empty) == 0, "committed empty value")
	check(c.Rollback())
	_, err := c.Commit()
	var rejected *silverhamu.ServerError
	require(errors.As(err, &rejected) && rejected.Code == "invalid_request", "missing server error code")
	must(c.DeleteCommit(private))
	must(c.PutCommit(key, value))
	fmt.Println("Go interoperation passed")
}
