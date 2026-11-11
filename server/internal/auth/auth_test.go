package auth

import (
	"testing"
	"time"
)

func TestHashVerify(t *testing.T) {
	h, err := HashPassword("correct horse")
	if err != nil {
		t.Fatal(err)
	}
	if !VerifyPassword(h, "correct horse") {
		t.Fatal("expected match")
	}
	if VerifyPassword(h, "wrong horse") {
		t.Fatal("expected mismatch")
	}
	if VerifyPassword("garbage", "x") {
		t.Fatal("garbage hash must not verify")
	}
}

func TestValidateUsername(t *testing.T) {
	for _, ok := range []string{"a", "ab", "abc", "Player_1", "ABCDEFGHIJKLMNOPQRST"} {
		if ValidateUsername(ok) != nil {
			t.Errorf("%q should be valid", ok)
		}
	}
	for _, bad := range []string{"", "has space", "émile", "ABCDEFGHIJKLMNOPQRSTU", "a-b"} {
		if ValidateUsername(bad) == nil {
			t.Errorf("%q should be invalid", bad)
		}
	}
}

func TestLimiter(t *testing.T) {
	l := NewLimiter(60, 3)
	now := time.Now()
	for i := 0; i < 3; i++ {
		if !l.allowAt("ip", now) {
			t.Fatalf("attempt %d should pass", i)
		}
	}
	if l.allowAt("ip", now) {
		t.Fatal("burst exceeded should fail")
	}
	if !l.allowAt("ip", now.Add(1100*time.Millisecond)) {
		t.Fatal("refill should allow")
	}
	if !l.allowAt("other", now) {
		t.Fatal("keys are independent")
	}
}
