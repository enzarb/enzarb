package main

import (
	"net/http"
	"testing"
)

func TestClientIP(t *testing.T) {
	tests := []struct {
		name    string
		headers map[string]string
		remote  string
		want    string
	}{
		{
			name:    "envoy external address wins",
			headers: map[string]string{"X-Envoy-External-Address": "10.42.0.5", "X-Forwarded-For": "1.2.3.4, 10.42.0.5"},
			want:    "10.42.0.5",
		},
		{
			// A client-spoofed XFF gets the real peer appended last by Envoy, so
			// the rightmost entry is the trustworthy one.
			name:    "rightmost xff when no envoy header",
			headers: map[string]string{"X-Forwarded-For": "9.9.9.9, 10.42.0.7"},
			want:    "10.42.0.7",
		},
		{
			name:   "remote addr fallback",
			remote: "10.42.0.9:54321",
			want:   "10.42.0.9",
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			r := &http.Request{Header: http.Header{}, RemoteAddr: tt.remote}
			for k, v := range tt.headers {
				r.Header.Set(k, v)
			}
			if got := clientIP(r); got != tt.want {
				t.Errorf("clientIP() = %q, want %q", got, tt.want)
			}
		})
	}
}
