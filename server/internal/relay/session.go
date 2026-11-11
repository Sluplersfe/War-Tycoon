package relay

import (
	"context"
	"errors"
	"net"
	"sync"
	"sync/atomic"
	"time"

	"frontline/internal/auth"
	"frontline/internal/pb"
	"frontline/internal/rooms"
	"frontline/internal/store"
)

// Session is one TCP connection. It implements rooms.Peer.
type Session struct {
	srv    *Server
	conn   net.Conn
	out    chan *pb.ServerMsg
	done   chan struct{}
	once   sync.Once
	connID uint32
	udpKey uint64

	userID   atomic.Uint64
	username atomic.Pointer[string]
	udpAddr  atomic.Pointer[net.UDPAddr]
	room     atomic.Pointer[rooms.Room]
}

func (s *Session) UserID() uint64 { return s.userID.Load() }
func (s *Session) Username() string {
	if p := s.username.Load(); p != nil {
		return *p
	}
	return ""
}
func (s *Session) ConnID() uint32 { return s.connID }
func (s *Session) UDPKey() uint64 { return s.udpKey }

// Send queues a message; a client that cannot keep up is disconnected.
func (s *Session) Send(m *pb.ServerMsg) {
	select {
	case <-s.done:
	case s.out <- m:
	default:
		s.srv.Log.Warn("send queue full, dropping client", "user", s.Username())
		s.close()
	}
}

func (s *Session) SendUDP(b []byte) {
	if a := s.udpAddr.Load(); a != nil {
		s.srv.udp.WriteToUDP(b, a)
	}
}

func (s *Session) LeftRoom(r *rooms.Room, reason string) {
	if s.room.CompareAndSwap(r, nil) {
		s.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_RoomLeft{RoomLeft: &pb.RoomLeft{Reason: reason}}})
	}
}

func (s *Session) close() {
	s.once.Do(func() {
		close(s.done)
		s.conn.Close()
	})
}

func (s *Session) remoteIP() string {
	if a, ok := s.conn.RemoteAddr().(*net.TCPAddr); ok {
		return a.IP.String()
	}
	return s.conn.RemoteAddr().String()
}

func (s *Session) run() {
	defer func() {
		s.close()
		if r := s.room.Swap(nil); r != nil {
			r.Leave(s)
		}
		s.srv.dropSession(s)
	}()
	go s.writer()
	for {
		s.conn.SetReadDeadline(time.Now().Add(readTimeout))
		msg := &pb.ClientMsg{}
		if err := ReadFrame(s.conn, msg); err != nil {
			return
		}
		s.handle(msg)
	}
}

func (s *Session) writer() {
	for {
		select {
		case <-s.done:
			return
		case m := <-s.out:
			s.conn.SetWriteDeadline(time.Now().Add(writeTimeout))
			if err := WriteFrame(s.conn, m); err != nil {
				s.close()
				return
			}
		}
	}
}

func (s *Session) fail(code string, err error) {
	s.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_Error{Error: &pb.Error{Code: code, Message: err.Error()}}})
}

var errInternal = errors.New("internal error")

func (s *Session) handle(msg *pb.ClientMsg) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	switch m := msg.Msg.(type) {
	case *pb.ClientMsg_Ping:
		s.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_Pong{Pong: &pb.Pong{
			ClientTimeMs: m.Ping.ClientTimeMs, ServerTimeMs: time.Now().UnixMilli(),
		}}})
		return
	case *pb.ClientMsg_Register:
		s.register(ctx, m.Register)
		return
	case *pb.ClientMsg_Login:
		s.login(ctx, m.Login)
		return
	case *pb.ClientMsg_Resume:
		s.resume(ctx, m.Resume)
		return
	}

	uid := s.UserID()
	if uid == 0 {
		s.fail("auth", errors.New("log in first"))
		return
	}

	switch m := msg.Msg.(type) {
	case *pb.ClientMsg_ListRooms:
		list, err := s.srv.Rooms.List(ctx, uid, m.ListRooms.IncludeArchived)
		if err != nil {
			s.srv.Log.Error("list rooms", "err", err)
			s.fail("rooms", errInternal)
			return
		}
		s.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_RoomList{RoomList: list}})
	case *pb.ClientMsg_CreateRoom:
		if !s.srv.roomLim.Allow(s.remoteIP()) {
			s.fail("rate", errors.New("too many rooms created, wait a minute"))
			return
		}
		sum, err := s.srv.Rooms.Create(ctx, uid, m.CreateRoom.Name, m.CreateRoom.Settings)
		if err != nil {
			s.fail("create_room", err)
			return
		}
		s.join(ctx, sum.Id)
	case *pb.ClientMsg_JoinRoom:
		s.join(ctx, m.JoinRoom.RoomId)
	case *pb.ClientMsg_LeaveRoom:
		if r := s.room.Swap(nil); r != nil {
			r.Leave(s)
			s.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_RoomLeft{RoomLeft: &pb.RoomLeft{}}})
		}
	case *pb.ClientMsg_Admin:
		if un, ok := m.Admin.Cmd.(*pb.AdminCommand_UnarchiveRoom); ok {
			if err := s.srv.Rooms.Unarchive(ctx, uid, un.UnarchiveRoom); err != nil {
				s.fail("admin", err)
				return
			}
			list, err := s.srv.Rooms.List(ctx, uid, true)
			if err == nil {
				s.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_RoomList{RoomList: list}})
			}
			return
		}
		s.inRoom(msg)
	default:
		s.inRoom(msg)
	}
}

func (s *Session) inRoom(msg *pb.ClientMsg) {
	r := s.room.Load()
	if r == nil {
		s.fail("room", errors.New("not in a room"))
		return
	}
	r.Handle(s, msg)
}

func (s *Session) join(ctx context.Context, roomID uint64) {
	if r := s.room.Swap(nil); r != nil {
		r.Leave(s)
	}
	r, err := s.srv.Rooms.Join(ctx, s, roomID)
	if err != nil {
		s.fail("join_room", err)
		return
	}
	s.room.Store(r)
}

func (s *Session) authed(ctx context.Context, u store.User) {
	token, hash, err := auth.NewToken()
	if err != nil {
		s.fail("auth", errInternal)
		return
	}
	if err := s.srv.Store.CreateSession(ctx, u.ID, hash, time.Now().Add(auth.SessionTTL)); err != nil {
		s.srv.Log.Error("create session", "err", err)
		s.fail("auth", errInternal)
		return
	}
	s.setUser(u)
	s.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_AuthOk{AuthOk: &pb.AuthOk{UserId: u.ID, Username: u.Username, Token: token}}})
}

func (s *Session) setUser(u store.User) {
	name := u.Username
	s.username.Store(&name)
	s.userID.Store(u.ID)
	s.srv.Log.Info("auth", "user", u.Username, "ip", s.remoteIP())
}

func (s *Session) hash(pw string) (string, error) {
	s.srv.hashSem <- struct{}{}
	defer func() { <-s.srv.hashSem }()
	return auth.HashPassword(pw)
}

func (s *Session) verify(hash, pw string) bool {
	s.srv.hashSem <- struct{}{}
	defer func() { <-s.srv.hashSem }()
	return auth.VerifyPassword(hash, pw)
}

func (s *Session) register(ctx context.Context, m *pb.Register) {
	if s.UserID() != 0 {
		s.fail("auth", errors.New("already logged in"))
		return
	}
	if !s.srv.authLim.Allow(s.remoteIP()) {
		s.fail("rate", errors.New("too many attempts, wait a minute"))
		return
	}
	if err := auth.ValidateUsername(m.Username); err != nil {
		s.fail("register", err)
		return
	}
	h, err := s.hash(m.Password)
	if err != nil {
		s.fail("register", errInternal)
		return
	}
	id, err := s.srv.Store.CreateUser(ctx, m.Username, h)
	if errors.Is(err, store.ErrUsernameTaken) {
		s.fail("register", err)
		return
	}
	if err != nil {
		s.srv.Log.Error("create user", "err", err)
		s.fail("register", errInternal)
		return
	}
	s.authed(ctx, store.User{ID: id, Username: m.Username})
}

var errBadLogin = errors.New("wrong username or password")

func (s *Session) login(ctx context.Context, m *pb.Login) {
	if s.UserID() != 0 {
		s.fail("auth", errors.New("already logged in"))
		return
	}
	if !s.srv.authLim.Allow(s.remoteIP()) {
		s.fail("rate", errors.New("too many attempts, wait a minute"))
		return
	}
	u, err := s.srv.Store.UserByName(ctx, m.Username)
	if err != nil {
		if !errors.Is(err, store.ErrNotFound) {
			s.srv.Log.Error("login lookup", "err", err)
		}
		s.fail("login", errBadLogin)
		return
	}
	if !s.verify(u.PasswordHash, m.Password) {
		s.fail("login", errBadLogin)
		return
	}
	s.authed(ctx, u)
}

func (s *Session) resume(ctx context.Context, m *pb.Resume) {
	if s.UserID() != 0 {
		s.fail("auth", errors.New("already logged in"))
		return
	}
	id, err := s.srv.Store.SessionUser(ctx, auth.HashToken(m.Token))
	if err != nil {
		s.fail("resume", errors.New("session expired, log in again"))
		return
	}
	u, err := s.srv.Store.UserByID(ctx, id)
	if err != nil {
		s.fail("resume", errors.New("session expired, log in again"))
		return
	}
	s.setUser(u)
	s.Send(&pb.ServerMsg{Msg: &pb.ServerMsg_AuthOk{AuthOk: &pb.AuthOk{UserId: u.ID, Username: u.Username, Token: m.Token}}})
}
