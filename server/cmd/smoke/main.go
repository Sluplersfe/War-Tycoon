// Command smoke checks a running server: TCP ping, register/login, create a
// room, and a UDP hello round trip on the realtime port.
package main

import (
	"flag"
	"fmt"
	"math/rand"
	"net"
	"os"
	"time"

	"google.golang.org/protobuf/proto"

	"frontline/internal/pb"
	"frontline/internal/relay"
)

func main() {
	host := flag.String("host", "76.179.76.99", "server host")
	tcpPort := flag.Int("tcp", 4002, "TCP port")
	udpPort := flag.Int("udp", 4003, "UDP port")
	flag.Parse()

	fail := func(step string, err error) {
		fmt.Printf("FAIL %s: %v\n", step, err)
		os.Exit(1)
	}
	tc, err := net.DialTimeout("tcp", fmt.Sprintf("%s:%d", *host, *tcpPort), 5*time.Second)
	if err != nil {
		fail("tcp connect", err)
	}
	defer tc.Close()
	tc.SetDeadline(time.Now().Add(15 * time.Second))
	call := func(m *pb.ClientMsg, want func(*pb.ServerMsg) bool) *pb.ServerMsg {
		if err := relay.WriteFrame(tc, m); err != nil {
			fail("write", err)
		}
		for {
			r := &pb.ServerMsg{}
			if err := relay.ReadFrame(tc, r); err != nil {
				fail("read", err)
			}
			if e := r.GetError(); e != nil {
				fail(e.Code, fmt.Errorf("%s", e.Message))
			}
			if want(r) {
				return r
			}
		}
	}

	t0 := time.Now()
	call(&pb.ClientMsg{Msg: &pb.ClientMsg_Ping{Ping: &pb.Ping{ClientTimeMs: t0.UnixMilli()}}},
		func(m *pb.ServerMsg) bool { return m.GetPong() != nil })
	fmt.Printf("ok   tcp ping %v\n", time.Since(t0).Round(time.Millisecond))

	name := fmt.Sprintf("smoke_%06d", rand.Intn(1000000))
	call(&pb.ClientMsg{Msg: &pb.ClientMsg_Register{Register: &pb.Register{Username: name, Password: "smoke-test-pw"}}},
		func(m *pb.ServerMsg) bool { return m.GetAuthOk() != nil })
	fmt.Printf("ok   registered %s\n", name)

	j := call(&pb.ClientMsg{Msg: &pb.ClientMsg_CreateRoom{CreateRoom: &pb.CreateRoom{Name: "Smoke test " + name}}},
		func(m *pb.ServerMsg) bool { return m.GetRoomJoined() != nil }).GetRoomJoined()
	fmt.Printf("ok   created room %d (plot %d, avatar %d)\n", j.Room.Id, j.You.Plot, j.You.AvatarId)

	uc, err := net.Dial("udp", fmt.Sprintf("%s:%d", *host, *udpPort))
	if err != nil {
		fail("udp dial", err)
	}
	defer uc.Close()
	hello, _ := proto.Marshal(&pb.UdpPacket{ConnId: j.ConnId, Key: j.UdpKey, Hello: true})
	buf := make([]byte, 2048)
	for i := 0; i < 5; i++ {
		t1 := time.Now()
		uc.Write(hello)
		uc.SetReadDeadline(time.Now().Add(time.Second))
		n, err := uc.Read(buf)
		if err != nil {
			continue
		}
		p := &pb.UdpPacket{}
		if proto.Unmarshal(buf[:n], p) == nil && p.Hello {
			fmt.Printf("ok   udp hello %v\n", time.Since(t1).Round(time.Millisecond))
			// Archive the throwaway room (rooms are never deleted).
			relay.WriteFrame(tc, &pb.ClientMsg{Msg: &pb.ClientMsg_Admin{Admin: &pb.AdminCommand{Cmd: &pb.AdminCommand_Archive{Archive: true}}}})
			time.Sleep(200 * time.Millisecond)
			return
		}
	}
	fail("udp hello", fmt.Errorf("no reply on %s:%d (check firewall/port forward for UDP)", *host, *udpPort))
}
