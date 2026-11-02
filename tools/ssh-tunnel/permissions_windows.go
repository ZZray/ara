// Copyright 2026 Ray Zhang.
//go:build windows

package main

import (
	"fmt"
	"os/exec"
	"os/user"
)

func secureDir(path string) error {
	current, err := user.Current()
	if err != nil {
		return err
	}
	// SID-based grants avoid localized account/group names. The caller owns this
	// newly created session directory, never an existing user's SSH directory.
	output, err := exec.Command("icacls.exe", path, "/inheritance:r", "/grant:r", "*"+current.Uid+":(OI)(CI)F", "*S-1-5-18:(OI)(CI)F").CombinedOutput()
	if err != nil {
		return fmt.Errorf("protect session directory: %w: %s", err, output)
	}
	return nil
}
