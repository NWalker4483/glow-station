use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Chart, Clear, Dataset, Gauge, GraphType, List, ListItem, ListState,
        Paragraph, Row, Table, Wrap,
    },
};
use std::{
    collections::VecDeque,
    error::Error,
    io,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

// Import your TEC controller code (assuming it's in the same crate)
use crate::tec::{TecConfig, TecController, TecReadout};

#[derive(Debug, Clone)]
struct TempData {
    timestamp: f64,
    set_temp: f32,
    measured_temp: f32,
}

#[derive(PartialEq)]
enum InputMode {
    Normal,
    Editing,
}

#[derive(PartialEq)]
enum SelectedPane {
    Parameters,
    Controls,
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum EditField {
    TSet,
    P,
    I,
    D,
    TMin,
    TMax,
}

struct App {
    // Data
    current_readout: Option<TecReadout>,
    current_config: TecConfig,
    temp_history: VecDeque<TempData>,

    // UI State
    input_mode: InputMode,
    selected_pane: SelectedPane,
    edit_field: EditField,
    edit_value: String,
    parameter_list_state: ListState,

    // Status
    tec_enabled: bool,
    last_update: Instant,
    error_message: Option<String>,

    // Controller
    controller: Arc<Mutex<TecController>>,
}

impl App {
    fn new(port_name: &str) -> Result<App, Box<dyn Error>> {
        let controller = TecController::new(port_name)?;

        let mut app = App {
            current_readout: None,
            current_config: TecConfig {
                t_set: 25.0,
                p: 1.0,
                i: 0.1,
                d: 0.0,
                t_min: 0.0,
                t_max: 70.0,
            },
            temp_history: VecDeque::with_capacity(1000),
            input_mode: InputMode::Normal,
            selected_pane: SelectedPane::Parameters,
            edit_field: EditField::TSet,
            edit_value: String::new(),
            parameter_list_state: ListState::default(),
            tec_enabled: false,
            last_update: Instant::now(),
            error_message: None,
            controller: Arc::new(Mutex::new(controller)),
        };

        // Select first parameter by default
        app.parameter_list_state.select(Some(0));

        Ok(app)
    }

    fn update_data(&mut self) {
        let controller = Arc::clone(&self.controller);

        if let Ok(mut ctrl) = controller.try_lock() {
            match ctrl.get_single_readout() {
                Ok(readout) => {
                    // Add to history
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs_f64();

                    self.temp_history.push_back(TempData {
                        timestamp: now,
                        set_temp: readout.t_set,
                        measured_temp: readout.t_measured,
                    });

                    // Keep only last 1000 points
                    if self.temp_history.len() > 1000 {
                        self.temp_history.pop_front();
                    }

                    self.current_readout = Some(readout);
                    self.error_message = None;
                    self.last_update = Instant::now();
                }
                Err(e) => {
                    self.error_message = Some(format!("Read error: {}", e));
                }
            }
        }
    }

    fn apply_configuration(&mut self) {
        let controller = Arc::clone(&self.controller);

        if let Ok(mut ctrl) = controller.try_lock() {
            match ctrl.set_configuration(&self.current_config) {
                Ok(_) => {
                    self.error_message = Some("Configuration updated".to_string());
                }
                Err(e) => {
                    self.error_message = Some(format!("Config error: {}", e));
                }
            }
        }
    }

    fn toggle_tec(&mut self) {
        let controller = Arc::clone(&self.controller);

        if let Ok(mut ctrl) = controller.try_lock() {
            let result = if self.tec_enabled {
                ctrl.disable()
            } else {
                ctrl.enable()
            };

            match result {
                Ok(_) => {
                    self.tec_enabled = !self.tec_enabled;
                    self.error_message = Some(format!(
                        "TEC {}",
                        if self.tec_enabled {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    ));
                }
                Err(e) => {
                    self.error_message = Some(format!("TEC control error: {}", e));
                }
            }
        }
    }

    fn handle_key_input(&mut self, key: KeyCode) {
        match self.input_mode {
            InputMode::Normal => {
                match key {
                    KeyCode::Char('q') => {
                        // Will be handled by main loop
                    }
                    KeyCode::Tab => {
                        self.selected_pane = match self.selected_pane {
                            SelectedPane::Parameters => SelectedPane::Controls,
                            SelectedPane::Controls => SelectedPane::Parameters,
                        };
                    }
                    KeyCode::Up => {
                        if self.selected_pane == SelectedPane::Parameters {
                            let i = match self.parameter_list_state.selected() {
                                Some(i) => {
                                    if i == 0 {
                                        5
                                    } else {
                                        i - 1
                                    }
                                }
                                None => 0,
                            };
                            self.parameter_list_state.select(Some(i));
                            self.edit_field = match i {
                                0 => EditField::TSet,
                                1 => EditField::P,
                                2 => EditField::I,
                                3 => EditField::D,
                                4 => EditField::TMin,
                                5 => EditField::TMax,
                                _ => EditField::TSet,
                            };
                        }
                    }
                    KeyCode::Down => {
                        if self.selected_pane == SelectedPane::Parameters {
                            let i = match self.parameter_list_state.selected() {
                                Some(i) => {
                                    if i >= 5 {
                                        0
                                    } else {
                                        i + 1
                                    }
                                }
                                None => 0,
                            };
                            self.parameter_list_state.select(Some(i));
                            self.edit_field = match i {
                                0 => EditField::TSet,
                                1 => EditField::P,
                                2 => EditField::I,
                                3 => EditField::D,
                                4 => EditField::TMin,
                                5 => EditField::TMax,
                                _ => EditField::TSet,
                            };
                        }
                    }
                    KeyCode::Enter => {
                        if self.selected_pane == SelectedPane::Parameters {
                            self.input_mode = InputMode::Editing;
                            self.edit_value = match self.edit_field {
                                EditField::TSet => self.current_config.t_set.to_string(),
                                EditField::P => self.current_config.p.to_string(),
                                EditField::I => self.current_config.i.to_string(),
                                EditField::D => self.current_config.d.to_string(),
                                EditField::TMin => self.current_config.t_min.to_string(),
                                EditField::TMax => self.current_config.t_max.to_string(),
                            };
                        } else {
                            self.toggle_tec();
                        }
                    }
                    KeyCode::Char('a') => {
                        self.apply_configuration();
                    }
                    _ => {}
                }
            }
            InputMode::Editing => match key {
                KeyCode::Enter => {
                    if let Ok(value) = self.edit_value.parse::<f32>() {
                        match self.edit_field {
                            EditField::TSet => self.current_config.t_set = value,
                            EditField::P => self.current_config.p = value,
                            EditField::I => self.current_config.i = value,
                            EditField::D => self.current_config.d = value,
                            EditField::TMin => self.current_config.t_min = value,
                            EditField::TMax => self.current_config.t_max = value,
                        }
                    } else {
                        self.error_message = Some("Invalid value entered".to_string());
                    }
                    self.input_mode = InputMode::Normal;
                    self.edit_value.clear();
                }
                KeyCode::Esc => {
                    self.input_mode = InputMode::Normal;
                    self.edit_value.clear();
                }
                KeyCode::Char(c) => {
                    self.edit_value.push(c);
                }
                KeyCode::Backspace => {
                    self.edit_value.pop();
                }
                _ => {}
            },
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(10),   // Main content
            Constraint::Length(3), // Status bar
        ])
        .split(f.size());

    // Title
    let title = Paragraph::new("TEC Controller Monitor")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // Main content layout
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30), // Parameters and controls
            Constraint::Percentage(40), // Chart
            Constraint::Percentage(30), // Current readings
        ])
        .split(chunks[1]);

    // Left panel - Parameters and Controls
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(main_chunks[0]);

    // Parameters
    let parameter_items: Vec<ListItem> = vec![
        ListItem::new(format!("Set Temp: {:.1}°C", app.current_config.t_set)),
        ListItem::new(format!("P: {:.3}", app.current_config.p)),
        ListItem::new(format!("I: {:.3}", app.current_config.i)),
        ListItem::new(format!("D: {:.3}", app.current_config.d)),
        ListItem::new(format!("T Min: {:.1}°C", app.current_config.t_min)),
        ListItem::new(format!("T Max: {:.1}°C", app.current_config.t_max)),
    ];

    let parameters = List::new(parameter_items)
        .block(
            Block::default()
                .title("Parameters (↑↓ to select, Enter to edit)")
                .borders(Borders::ALL)
                .border_style(if app.selected_pane == SelectedPane::Parameters {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                }),
        )
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");

    f.render_stateful_widget(parameters, left_chunks[0], &mut app.parameter_list_state);

    // Controls
    let control_text = vec![
        Line::from(vec![
            Span::styled("TEC Status: ", Style::default().fg(Color::White)),
            Span::styled(
                if app.tec_enabled { "ON" } else { "OFF" },
                Style::default().fg(if app.tec_enabled {
                    Color::Green
                } else {
                    Color::Red
                }),
            ),
        ]),
        Line::from(""),
        Line::from("Enter: Toggle TEC"),
        Line::from("'a': Apply Config"),
        Line::from("Tab: Switch Panes"),
        Line::from("'q': Quit"),
    ];

    let controls = Paragraph::new(control_text)
        .block(
            Block::default()
                .title("Controls")
                .borders(Borders::ALL)
                .border_style(if app.selected_pane == SelectedPane::Controls {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                }),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(controls, left_chunks[1]);

    // Center panel - Temperature Chart
    if !app.temp_history.is_empty() {
        let min_time = app.temp_history.front().unwrap().timestamp;
        let max_time = app.temp_history.back().unwrap().timestamp;

        let set_data: Vec<(f64, f64)> = app
            .temp_history
            .iter()
            .map(|data| (data.timestamp - min_time, data.set_temp as f64))
            .collect();

        let measured_data: Vec<(f64, f64)> = app
            .temp_history
            .iter()
            .map(|data| (data.timestamp - min_time, data.measured_temp as f64))
            .collect();

        let datasets = vec![
            Dataset::default()
                .name("Set Temp")
                .marker(symbols::Marker::Dot)
                .style(Style::default().fg(Color::Red))
                .graph_type(GraphType::Line)
                .data(&set_data),
            Dataset::default()
                .name("Measured Temp")
                .marker(symbols::Marker::Braille)
                .style(Style::default().fg(Color::Blue))
                .graph_type(GraphType::Line)
                .data(&measured_data),
        ];

        // Calculate temperature bounds
        let all_temps: Vec<f64> = app
            .temp_history
            .iter()
            .flat_map(|data| vec![data.set_temp as f64, data.measured_temp as f64])
            .collect();

        let min_temp = all_temps.iter().fold(f64::INFINITY, |a, &b| a.min(b)) - 2.0;
        let max_temp = all_temps.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b)) + 2.0;

        let chart = Chart::new(datasets)
            .block(
                Block::default()
                    .title("Temperature Plot")
                    .borders(Borders::ALL),
            )
            .x_axis(
                Axis::default()
                    .title("Time (s)")
                    .style(Style::default().fg(Color::Gray))
                    .bounds([0.0, max_time - min_time]),
            )
            .y_axis(
                Axis::default()
                    .title("Temperature (°C)")
                    .style(Style::default().fg(Color::Gray))
                    .bounds([min_temp, max_temp]),
            );

        f.render_widget(chart, main_chunks[1]);
    } else {
        let no_data = Paragraph::new("No data available yet...")
            .block(
                Block::default()
                    .title("Temperature Plot")
                    .borders(Borders::ALL),
            )
            .alignment(Alignment::Center);
        f.render_widget(no_data, main_chunks[1]);
    }

    // Right panel - Current Readings
    if let Some(readout) = &app.current_readout {
        let pwm_percentage = ((readout.PWM.abs() / 100.0) * 100.0).clamp(0.0, 100.0) as u16;

        let readings_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Current temp
                Constraint::Length(3), // PWM gauge
                Constraint::Min(5),    // Details table
            ])
            .split(main_chunks[2]);

        // Current temperature display
        let temp_diff = readout.t_measured - readout.t_set;
        let temp_color = if temp_diff.abs() < 0.5 {
            Color::Green
        } else if temp_diff.abs() < 2.0 {
            Color::Yellow
        } else {
            Color::Red
        };

        let current_temp = Paragraph::new(format!("{:.1}°C", readout.t_measured))
            .style(Style::default().fg(temp_color).add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title("Current Temperature")
                    .borders(Borders::ALL),
            );

        f.render_widget(current_temp, readings_chunks[0]);

        // PWM Gauge
        let pwm_label = if readout.PWM >= 0.0 {
            "Heating"
        } else {
            "Cooling"
        };
        let pwm_color = if readout.PWM >= 0.0 {
            Color::Red
        } else {
            Color::Blue
        };

        let pwm_gauge = Gauge::default()
            .block(
                Block::default()
                    .title(format!("PWM - {}", pwm_label))
                    .borders(Borders::ALL),
            )
            .gauge_style(Style::default().fg(pwm_color))
            .percent(pwm_percentage)
            .label(format!("{:.1}%", readout.PWM));

        f.render_widget(pwm_gauge, readings_chunks[1]);

        // Details table
        let t_range_str = format!("{:.1}-{:.1}°C", readout.t_min, readout.t_max);
        let set_temp_str = format!("{:.1}°C", readout.t_set);
        let measured_str = format!("{:.1}°C", readout.t_measured);
        let p_str = format!("{:.3}", readout.P);
        let i_str = format!("{:.3}", readout.I);
        let d_str = format!("{:.3}", readout.D);

        let rows = vec![
            Row::new(vec!["Set Temp", &set_temp_str]),
            Row::new(vec!["Measured", &measured_str]),
            Row::new(vec!["P", &p_str]),
            Row::new(vec!["I", &i_str]),
            Row::new(vec!["D", &d_str]),
            Row::new(vec!["T Range", &t_range_str]),
            Row::new(vec![
                "OC Status",
                if readout.OC {
                    "Connected"
                } else {
                    "Disconnected"
                },
            ]),
        ];

        let details = Table::new(rows, [Constraint::Length(10), Constraint::Min(10)])
            .block(Block::default().title("Details").borders(Borders::ALL))
            .column_spacing(1);
        f.render_widget(details, readings_chunks[2]);
    } else {
        let no_data = Paragraph::new("No readings available")
            .block(
                Block::default()
                    .title("Current Readings")
                    .borders(Borders::ALL),
            )
            .alignment(Alignment::Center);
        f.render_widget(no_data, main_chunks[2]);
    }

    // Status bar
    let mut status_text = vec![Span::styled(
        format!(
            "Last update: {:.1}s ago",
            app.last_update.elapsed().as_secs_f32()
        ),
        Style::default().fg(Color::Gray),
    )];

    if let Some(ref error) = app.error_message {
        status_text.push(Span::raw(" | "));
        status_text.push(Span::styled(error.clone(), Style::default().fg(Color::Red)));
    }

    let status =
        Paragraph::new(Line::from(status_text)).block(Block::default().borders(Borders::ALL));
    f.render_widget(status, chunks[2]);

    // Edit mode popup
    if app.input_mode == InputMode::Editing {
        let area = centered_rect(40, 7, f.size());
        f.render_widget(Clear, area);

        let edit_title = format!("Edit {:?}", app.edit_field);
        let edit_popup = Paragraph::new(app.edit_value.as_str())
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .title(edit_title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            );

        f.render_widget(edit_popup, area);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub fn run_tui(port_name: &str) -> Result<(), Box<dyn Error>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new(port_name)?;

    // Main loop
    let mut last_update = Instant::now();
    loop {
        // Update data every 500ms
        if last_update.elapsed() > Duration::from_millis(500) {
            app.update_data();
            last_update = Instant::now();
        }

        // Draw UI
        terminal.draw(|f| ui(f, &mut app))?;

        // Handle input with timeout
        if crossterm::event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        _ => app.handle_key_input(key.code),
                    }
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

// Example main function - you can remove this if integrating into existing crate
// fn main() -> Result<(), Box<dyn Error>> {
//     let args: Vec<String> = std::env::args().collect();
//     if args.len() != 2 {
//         eprintln!("Usage: {} <serial_port>", args[0]);
//         eprintln!("Example: {} /dev/ttyUSB0", args[0]);
//         std::process::exit(1);
//     }
//
//     run_tui(&args[1])
// }
