package main

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/charmbracelet/bubbles/list"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/muesli/termenv"

	"github.com/Noswad123/djinn/internal/parser"
	"github.com/Noswad123/djinn/internal/styles"
	"github.com/Noswad123/djinn/internal/ui"
)

func main() {
	home, _ := os.UserHomeDir()
	dotfilesPath := filepath.Join(home, ".dotfiles")
	lipgloss.SetColorProfile(termenv.TrueColor)

	items, err := parser.ScanDotfiles(dotfilesPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error scanning dotfiles: %v\n", err)
		os.Exit(1)
	}

	if len(items) == 0 {
		fmt.Fprintf(os.Stderr, "Djinn found 0 tags. Check your @name format in %s\n", dotfilesPath)
		os.Exit(1)
	}

	m := ui.Model{
		List:   list.New(items, list.NewDefaultDelegate(), 0, 0),
		Styles: styles.DefaultStyles(),
	}
	m.List.Title = "Your custom functions..."
	m.List.SetShowStatusBar(false)

	p := tea.NewProgram(&m, tea.WithAltScreen(), tea.WithOutput(os.Stderr))

	finalModel, err := p.Run()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error running Djinn: %v\n", err)
		os.Exit(1)
	}

	if choice := finalModel.(*ui.Model).Choice; choice != "" {
		fmt.Print(choice)
	}
}
