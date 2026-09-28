package valka

import (
	"context"
	"errors"
	"testing"
)

type recordedCheckpoint struct{ step, output string }

func newTestContext(existing map[string]string, fail error) (*TaskContext, *[]recordedCheckpoint) {
	var saved []recordedCheckpoint
	checkpoints := map[string]string{}
	for k, v := range existing {
		checkpoints[k] = v
	}
	return &TaskContext{
		Context: context.Background(),
		checkpointFn: func(_ context.Context, step, output string) error {
			if fail != nil {
				return fail
			}
			saved = append(saved, recordedCheckpoint{step, output})
			return nil
		},
		checkpoints: checkpoints,
	}, &saved
}

func TestStepSkipsCheckpointedStep(t *testing.T) {
	ctx, saved := newTestContext(map[string]string{"fetch": `{"rows":3}`}, nil)
	type rows struct {
		Rows int `json:"rows"`
	}
	ran := 0
	got, err := Step(ctx, "fetch", func() (rows, error) {
		ran++
		return rows{Rows: 99}, nil
	})
	if err != nil || got.Rows != 3 || ran != 0 || len(*saved) != 0 {
		t.Fatalf("got=%+v err=%v ran=%d saved=%v", got, err, ran, *saved)
	}
}

func TestStepRunsAndCheckpointsNewStep(t *testing.T) {
	ctx, saved := newTestContext(nil, nil)
	got, err := Step(ctx, "double", func() (int, error) { return 42, nil })
	if err != nil || got != 42 {
		t.Fatalf("got=%d err=%v", got, err)
	}
	if len(*saved) != 1 || (*saved)[0] != (recordedCheckpoint{"double", "42"}) {
		t.Fatalf("saved=%v", *saved)
	}
	again, err := Step(ctx, "double", func() (int, error) { return 0, errors.New("must not run") })
	if err != nil || again != 42 {
		t.Fatalf("repeat step got=%d err=%v", again, err)
	}
}

func TestStepErrorIsNotCheckpointed(t *testing.T) {
	ctx, saved := newTestContext(nil, nil)
	boom := errors.New("boom")
	if _, err := Step(ctx, "parse", func() (int, error) { return 0, boom }); !errors.Is(err, boom) {
		t.Fatalf("err=%v", err)
	}
	if found, _ := ctx.CheckpointValue("parse", new(int)); found || len(*saved) != 0 {
		t.Fatalf("found=%v saved=%v", found, *saved)
	}
}

func TestStepFailsWhenCheckpointIsNotPersisted(t *testing.T) {
	down := errors.New("unavailable")
	ctx, _ := newTestContext(nil, down)
	if _, err := Step(ctx, "upload", func() (string, error) { return "ok", nil }); !errors.Is(err, down) {
		t.Fatalf("err=%v", err)
	}
	if found, _ := ctx.CheckpointValue("upload", new(string)); found {
		t.Fatal("unpersisted step must not be cached")
	}
}

func TestCheckpointValueDecodeError(t *testing.T) {
	ctx, _ := newTestContext(map[string]string{"ids": "[1,2]"}, nil)
	var s string
	found, err := ctx.CheckpointValue("ids", &s)
	if !found || err == nil {
		t.Fatalf("found=%v err=%v", found, err)
	}
}
