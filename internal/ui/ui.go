package ui

import (
	"fmt"
	"github.com/charmbracelet/bubbles/list"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/Noswad123/djinn/internal/parser"
	"github.com/Noswad123/djinn/internal/styles"
)

type Model struct {
	List     list.Model
	Viewport viewport.Model
	Styles   styles.Styles
	Choice   string
	Ready    bool
}

func (m Model) Init() tea.Cmd {
	return nil
}

func (m *Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmds []tea.Cmd

	switch msg := msg.(type) {
	case tea.KeyMsg:
		switch msg.String() {
		case "ctrl+c", "q", "esc":
			return m, tea.Quit
		case "ctrl+d":
			m.Viewport.ScrollDown(5)
		case "ctrl+u":
			m.Viewport.ScrollUp(5)
		case "enter":
			if i, ok := m.List.SelectedItem().(parser.Item); ok {
				m.Choice = fmt.Sprintf("%s:%d", i.Path, i.Line)
				return m, tea.Quit
			}
		}

	case tea.WindowSizeMsg:
		h, v := m.Styles.Doc.GetFrameSize()
		listWidth := 100
		m.List.SetSize(listWidth, msg.Height-v)

		if !m.Ready {
			m.Viewport = viewport.New(msg.Width-listWidth-h-4, msg.Height-v)
			m.Ready = true
		} else {
			m.Viewport.Width = msg.Width - listWidth - h - 4
			m.Viewport.Height = msg.Height - v
		}
	}

	var listCmd tea.Cmd
	m.List, listCmd = m.List.Update(msg)
	cmds = append(cmds, listCmd)

	if i, ok := m.List.SelectedItem().(parser.Item); ok {
		m.Viewport.SetContent(i.Preview)
	}

	return m, tea.Batch(cmds...)
}

func (m Model) View() string {
	if !m.Ready {
		return "\n  Initializing Djinn..."
	}

	listView := m.Styles.List.Width(35).Render(m.List.View())
	previewView := m.Styles.Preview.Render(m.Viewport.View())

	return m.Styles.Doc.Render(
		lipgloss.JoinHorizontal(lipgloss.Top, listView, previewView),
	)
}
