package styles

import "github.com/charmbracelet/lipgloss"

type Styles struct {
	Doc       lipgloss.Style
	List      lipgloss.Style
	Preview   lipgloss.Style
	Title     lipgloss.Style
	Help      lipgloss.Style
}

func DefaultStyles() Styles {
	s := Styles{}
	s.Doc = lipgloss.NewStyle().Margin(1, 2)
	s.List = lipgloss.NewStyle().
		Width(45).
		Border(lipgloss.NormalBorder(), false, true, false, false).
		BorderForeground(lipgloss.Color("240"))
	s.Preview = lipgloss.NewStyle().Padding(0, 2)
	s.Title = lipgloss.NewStyle().
		Background(lipgloss.Color("62")).
		Foreground(lipgloss.Color("230")).
		Padding(0, 1).
		Bold(true)
	return s
}
