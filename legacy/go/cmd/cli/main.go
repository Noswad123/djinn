package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/charmbracelet/bubbles/list"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/muesli/termenv"

	"github.com/Noswad123/djinn/internal/parser"
	"github.com/Noswad123/djinn/internal/styles"
	"github.com/Noswad123/djinn/internal/ui"
)

type indexEntry struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	Path        string `json:"path"`
	Line        int    `json:"line"`
}

type indexPayload struct {
	SchemaVersion int          `json:"schema_version"`
	Source        string       `json:"source"`
	Root          string       `json:"root"`
	Count         int          `json:"count"`
	Entries       []indexEntry `json:"entries"`
}

func defaultRoot() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".dotfiles")
}

func defaultIndexPath(root string) string {
	return filepath.Join(root, "opencode", ".config", "opencode", "djinn-index.json")
}

func defaultEditor() string {
	for _, key := range []string{"VISUAL", "EDITOR"} {
		if value := strings.TrimSpace(os.Getenv(key)); value != "" {
			return value
		}
	}

	return "nvim"
}

func openInEditor(item parser.Item, editor string) error {
	parts := strings.Fields(strings.TrimSpace(editor))
	if len(parts) == 0 {
		return fmt.Errorf("editor command is empty")
	}

	args := append([]string{}, parts[1:]...)
	args = append(args, fmt.Sprintf("+%d", item.Line), item.Path)

	cmd := exec.Command(parts[0], args...)
	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

func runSyncCache(root string, indexPath string) error {
	items, err := parser.ScanItems(root)
	if err != nil {
		return fmt.Errorf("error scanning dotfiles: %w", err)
	}

	entries := make([]indexEntry, 0, len(items))
	for _, item := range items {
		relPath, relErr := filepath.Rel(root, item.Path)
		if relErr != nil {
			relPath = item.Path
		}

		entries = append(entries, indexEntry{
			Name:        item.Name,
			Description: item.Summary,
			Path:        filepath.ToSlash(relPath),
			Line:        item.Line,
		})
	}

	payload := indexPayload{
		SchemaVersion: 1,
		Source:        "djinn-tag-scan",
		Root:          "~/.dotfiles",
		Count:         len(entries),
		Entries:       entries,
	}

	rendered, err := json.MarshalIndent(payload, "", "  ")
	if err != nil {
		return fmt.Errorf("error creating djinn index JSON: %w", err)
	}
	rendered = append(rendered, '\n')

	if existing, readErr := os.ReadFile(indexPath); readErr == nil {
		if string(existing) == string(rendered) {
			fmt.Fprintf(os.Stderr, "djinn sync-cache: unchanged (%s)\n", indexPath)
			return nil
		}
	}

	if err := os.MkdirAll(filepath.Dir(indexPath), 0o755); err != nil {
		return fmt.Errorf("error creating index directory: %w", err)
	}

	if err := os.WriteFile(indexPath, rendered, 0o644); err != nil {
		return fmt.Errorf("error writing djinn index: %w", err)
	}

	fmt.Fprintf(os.Stderr, "djinn sync-cache: updated %s (%d entries)\n", indexPath, len(entries))
	return nil
}

func main() {
	syncCache := flag.Bool("sync-cache", false, "scan tags and update cached djinn index JSON")
	root := flag.String("root", defaultRoot(), "dotfiles root path to scan")
	index := flag.String("index", "", "path to index JSON file (defaults under --root)")
	open := flag.Bool("open", false, "open the selected item in an editor instead of printing path:line")
	editor := flag.String("editor", "", "editor command for --open (defaults to $VISUAL, $EDITOR, then nvim; must accept +line file)")
	flag.Parse()

	indexPath := strings.TrimSpace(*index)
	if indexPath == "" {
		indexPath = defaultIndexPath(*root)
	}

	if *syncCache {
		if err := runSyncCache(*root, indexPath); err != nil {
			fmt.Fprintf(os.Stderr, "Error running sync-cache: %v\n", err)
			os.Exit(1)
		}
		return
	}

	lipgloss.SetColorProfile(termenv.TrueColor)

	items, err := parser.ScanDotfiles(*root)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error scanning dotfiles: %v\n", err)
		os.Exit(1)
	}

	if len(items) == 0 {
		fmt.Fprintf(os.Stderr, "Djinn found 0 tags. Check your @name format in %s\n", *root)
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

	result := finalModel.(*ui.Model)
	if result.Selected != nil && *open {
		editorCommand := strings.TrimSpace(*editor)
		if editorCommand == "" {
			editorCommand = defaultEditor()
		}

		if err := openInEditor(*result.Selected, editorCommand); err != nil {
			fmt.Fprintf(os.Stderr, "Error opening editor: %v\n", err)
			os.Exit(1)
		}
		return
	}

	if choice := result.Choice; choice != "" {
		fmt.Print(choice)
	}
}
