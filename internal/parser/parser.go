package parser

import (
	"bufio"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/charmbracelet/bubbles/list"
)

type Item struct {
	Name, Summary, Path, Preview string
	Line                         int
}

func (i Item) Title() string       { return i.Name }
func (i Item) Description() string { return i.Summary }
func (i Item) FilterValue() string { return i.Name + " " + i.Summary }

var skippedDirectories = map[string]struct{}{
	".git":         {},
	".opencode":    {},
	"node_modules": {},
	"dist":         {},
	".tmux":        {},
}

func shouldScan(path string) bool {
	ext := filepath.Ext(path)
	return ext == ".zsh" || ext == ".sh" || ext == ".lua"
}

func ScanItems(root string) ([]Item, error) {
	var items []Item

	err := filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil
		}

		if info.IsDir() {
			if _, skip := skippedDirectories[info.Name()]; skip {
				return filepath.SkipDir
			}
			return nil
		}

		if !shouldScan(path) {
			return nil
		}

		file, openErr := os.Open(path)
		if openErr != nil {
			return nil
		}
		defer file.Close()

		var fileContent []string
		scanner := bufio.NewScanner(file)
		for scanner.Scan() {
			fileContent = append(fileContent, scanner.Text())
		}

		var currentName string
		var nameLine int

		for i, line := range fileContent {
			cleanLine := strings.TrimSpace(line)

			if strings.Contains(cleanLine, "@name:") {
				currentName = strings.TrimSpace(strings.Split(cleanLine, "@name:")[1])
				nameLine = i
				continue
			}

			if strings.Contains(cleanLine, "@description:") && currentName != "" {
				desc := strings.TrimSpace(strings.Split(cleanLine, "@description:")[1])

				endIdx := i + 4
				for j := i + 1; j < len(fileContent); j++ {
					if strings.Contains(fileContent[j], "@end") {
						endIdx = j - 1
						break
					}
					if strings.Contains(fileContent[j], "@name:") {
						break
					}
				}

				if endIdx >= len(fileContent) {
					endIdx = len(fileContent) - 1
				}

				items = append(items, Item{
					Name:    currentName,
					Summary: desc,
					Path:    path,
					Line:    nameLine + 1,
					Preview: Highlight(strings.Join(fileContent[nameLine:endIdx+1], "\n"), path),
				})
				currentName = ""
			}
		}

		return nil
	})

	sort.Slice(items, func(i, j int) bool {
		left := strings.ToLower(items[i].Name)
		right := strings.ToLower(items[j].Name)
		if left == right {
			if items[i].Path == items[j].Path {
				return items[i].Line < items[j].Line
			}
			return items[i].Path < items[j].Path
		}
		return left < right
	})

	return items, err
}

func ScanDotfiles(root string) ([]list.Item, error) {
	parsedItems, err := ScanItems(root)
	if err != nil {
		return nil, err
	}

	listItems := make([]list.Item, 0, len(parsedItems))
	for _, item := range parsedItems {
		listItems = append(listItems, item)
	}

	return listItems, nil
}
