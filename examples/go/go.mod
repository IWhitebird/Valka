module github.com/valka-queue/valka/examples/go

go 1.21

require github.com/valka-queue/valka/sdks/go v0.0.0

require (
	golang.org/x/net v0.28.0 // indirect
	golang.org/x/sys v0.24.0 // indirect
	golang.org/x/text v0.17.0 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20240814211410-ddb44dafa142 // indirect
	google.golang.org/grpc v1.67.1 // indirect
	google.golang.org/protobuf v1.35.2 // indirect
)

replace github.com/valka-queue/valka/sdks/go => ../../sdks/go
