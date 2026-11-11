// Package relay serves TCP (reliable, length-prefixed protobuf) and UDP
// (realtime state) and routes messages into rooms.
package relay

import (
	"context"
	"crypto/rand"
	"encoding/binary"
	"errors"
	"log/slog"
	"net"
	"sync"
	"time"

	"google.golang.org/protobuf/proto"

	"frontline/internal/auth"
	"frontline/internal/pb"
	"frontline/internal/rooms"
	"frontline/internal/store"
)

const (
	maxFrame     = 1 << 20
	readTimeout  = 60 * time.Second
	writeTimeout = 10 * time.Second
	outQueue     = 512
)

type Server struct {
	Store   store.Store
	Rooms   *rooms.Manager
	Log     *slog.Logger
	authLim *auth.Limiter
	roomLim *auth.Limiter
	hashSem chan struct{}

	udp *net.UDPConn
	tcp net.Listener

	mu       sync.Mutex
	sessions map[uint32]*Session
	wg       sync.WaitGroup
}

func New(st store.Store, rm *rooms.Manager, log *slog.Logger) *Server {
	return &Server{
		Store: st, Rooms: rm, Log: log,
		authLim:  auth.NewLimiter(10, 5),
		roomLim:  auth.NewLimiter(5, 3),
		hashSem:  make(chan struct{}, 4),
		sessions: map[uint32]*Session{},
	}
}

// Listen binds both sockets. Serve must be called afterwards.
func (s *Server) Listen(tcpAddr, udpAddr string) error {
	l, err := net.Listen("tcp", tcpAddr)
	if err != nil {
		return err
	}
	ua, err := net.ResolveUDPAddr("udp", udpAddr)
	if err != nil {
		l.Close()
		return err
	}
	u, err := net.ListenUDP("udp", ua)
	if err != nil {
		l.Close()
		return err
	}
	_ = u.SetReadBuffer(4 << 20)
	_ = u.SetWriteBuffer(4 << 20)
	s.tcp, s.udp = l, u
	return nil
}

func (s *Server) TCPAddr() net.Addr { return s.tcp.Addr() }
func (s *Server) UDPAddr() net.Addr { return s.udp.LocalAddr() }

// Serve runs until ctx is cancelled.
func (s *Server) Serve(ctx context.Context) {
	go func() {
		<-ctx.Done()
		s.tcp.Close()
		s.udp.Close()
	}()
	s.wg.Add(1)
	go s.serveUDP()
	for {
		c, err := s.tcp.Accept()
		if err != nil {
			if ctx.Err() != nil || errors.Is(err, net.ErrClosed) {
				break
			}
			s.Log.Warn("accept", "err", err)
			time.Sleep(50 * time.Millisecond)
			continue
		}
		if tc, ok := c.(*net.TCPConn); ok {
			tc.SetNoDelay(true)
		}
		sess := s.newSession(c)
		s.wg.Add(1)
		go func() {
			defer s.wg.Done()
			sess.run()
		}()
	}
	s.mu.Lock()
	for _, sess := range s.sessions {
		sess.close()
	}
	s.mu.Unlock()
	s.wg.Wait()
}

func (s *Server) newSession(c net.Conn) *Session {
	sess := &Session{srv: s, conn: c, out: make(chan *pb.ServerMsg, outQueue), done: make(chan struct{})}
	var b [12]byte
	s.mu.Lock()
	defer s.mu.Unlock()
	for {
		rand.Read(b[:])
		id := binary.LittleEndian.Uint32(b[:4])
		if id != 0 && s.sessions[id] == nil {
			sess.connID = id
			sess.udpKey = binary.LittleEndian.Uint64(b[4:])
			s.sessions[id] = sess
			return sess
		}
	}
}

func (s *Server) dropSession(sess *Session) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.sessions[sess.connID] == sess {
		delete(s.sessions, sess.connID)
	}
}

func (s *Server) serveUDP() {
	defer s.wg.Done()
	buf := make([]byte, 2048)
	for {
		n, addr, err := s.udp.ReadFromUDP(buf)
		if err != nil {
			if errors.Is(err, net.ErrClosed) {
				return
			}
			continue
		}
		pkt := &pb.UdpPacket{}
		if err := proto.Unmarshal(buf[:n], pkt); err != nil {
			continue
		}
		s.mu.Lock()
		sess := s.sessions[pkt.ConnId]
		s.mu.Unlock()
		if sess == nil || sess.udpKey != pkt.Key || sess.userID.Load() == 0 {
			continue
		}
		// Track the latest source address (handles NAT rebinding).
		if cur := sess.udpAddr.Load(); cur == nil || !cur.IP.Equal(addr.IP) || cur.Port != addr.Port {
			a := *addr
			sess.udpAddr.Store(&a)
		}
		if pkt.Hello {
			ack, _ := proto.Marshal(&pb.UdpPacket{Hello: true, Seq: pkt.Seq})
			s.udp.WriteToUDP(ack, addr)
			continue
		}
		if r := sess.room.Load(); r != nil {
			r.HandleUDP(sess, pkt)
		}
	}
}

// WriteFrame writes one length-prefixed message.
func WriteFrame(w net.Conn, m proto.Message) error {
	b, err := proto.Marshal(m)
	if err != nil {
		return err
	}
	frame := make([]byte, 4+len(b))
	binary.BigEndian.PutUint32(frame, uint32(len(b)))
	copy(frame[4:], b)
	_, err = w.Write(frame)
	return err
}

// ReadFrame reads one length-prefixed message into m.
func ReadFrame(r net.Conn, m proto.Message) error {
	var hdr [4]byte
	if _, err := readFull(r, hdr[:]); err != nil {
		return err
	}
	n := binary.BigEndian.Uint32(hdr[:])
	if n > maxFrame {
		return errors.New("frame too large")
	}
	b := make([]byte, n)
	if _, err := readFull(r, b); err != nil {
		return err
	}
	return proto.Unmarshal(b, m)
}

func readFull(r net.Conn, b []byte) (int, error) {
	n := 0
	for n < len(b) {
		k, err := r.Read(b[n:])
		n += k
		if err != nil {
			return n, err
		}
	}
	return n, nil
}
