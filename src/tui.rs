use std::process;

mod tec;


use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Chart, Clear, Dataset, GraphType, List, ListItem, ListState,
        Paragraph, Wrap,
    },
};
use std::{
    collections::VecDeque,
    error::Error,
    io,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

// Import your TEC controller code
use crate::tec::{TecConfig, TecController, TecReadout};

fn main() {
    if let Err(e) = run_tui() {
        eprintln!("Error running TUI: {}", e);
        process::exit(1);
    }
}
#[derive(Debug, Clone)]
struct TempData {
    timestamp: f64,
    set_temp: f32,
    measured_temp: f32,
    pwm: f32,
}

#[derive(Debug, Clone)]
struct SetpointChange {
    target_temp: f32,
    start_time: Instant,
    reached_time: Option<Instant>,
    duration: Option<Duration>,
}

#[derive(PartialEq)]
enum InputMode {
    Normal,
    Editing,
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

impl EditField {
    fn get_step(&self, temp_step: f32) -> f32 {
        match self {
            EditField::TSet => temp_step,
            EditField::P | EditField::I | EditField::D => 0.01,
            EditField::TMin | EditField::TMax => 1.0,
        }
    }

    fn get_value(&self, config: &TecConfig) -> f32 {
        match self {
            EditField::TSet => config.t_set,
            EditField::P => config.p,
            EditField::I => config.i,
            EditField::D => config.d,
            EditField::TMin => config.t_min,
            EditField::TMax => config.t_max,
        }
    }

    fn label(&self) -> &str {
        match self {
            EditField::TSet => "Set Temp",
            EditField::P => "P Gain",
            EditField::I => "I Gain",
            EditField::D => "D Gain",
            EditField::TMin => "T Min",
            EditField::TMax => "T Max",
        }
    }

    fn from_index(i: usize) -> Self {
        match i {
            0 => EditField::TSet,
            1 => EditField::P,
            2 => EditField::I,
            3 => EditField::D,
            4 => EditField::TMin,
            5 => EditField::TMax,
            _ => EditField::TSet,
        }
    }
}

// Commands sent from UI thread to worker thread
enum WorkerCommand {
    SetConfig(TecConfig),
    Enable,
    Disable,
    Shutdown,
}

// Data sent from worker thread to UI thread
enum WorkerResponse {
    Readout(TecReadout),
    Error(String),
    Status(String),
}

struct App {
    // Data
    current_readout: Option<TecReadout>,
    current_config: TecConfig,
    temp_history: VecDeque<TempData>,

    // Temperature setpoint tracking
    current_setpoint_change: Option<SetpointChange>,
    setpoint_history: VecDeque<SetpointChange>,
    temp_tolerance: f32,

    // UI State
    input_mode: InputMode,
    edit_field: EditField,
    edit_value: String,
    parameter_list_state: ListState,

    // Status
    tec_enabled: bool,
    last_update: Instant,
    status_message: Option<String>,

    // Communication channels
    command_tx: Sender<WorkerCommand>,
    response_rx: Receiver<WorkerResponse>,

    // Settings
    temp_step: f32,

    // Redraw flag
    needs_redraw: bool,

    // Config debouncing
    pending_config: bool,
    last_config_sent: Instant,
}

impl App {
    fn new(port_name: &str) -> Result<App, Box<dyn Error>> {
        let (command_tx, command_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();

        // Spawn worker thread for serial communication
        let port_name = port_name.to_string();
        thread::spawn(move || {
            worker_thread(port_name, command_rx, response_tx);
        });

        let mut app = App {
            current_readout: None,
            current_config: TecConfig::default(),
            temp_history: VecDeque::with_capacity(1000),
            current_setpoint_change: None,
            setpoint_history: VecDeque::with_capacity(100),
            temp_tolerance: 0.5,
            input_mode: InputMode::Normal,
            edit_field: EditField::TSet,
            edit_value: String::new(),
            parameter_list_state: ListState::default(),
            tec_enabled: false,
            last_update: Instant::now(),
            status_message: None,
            command_tx,
            response_rx,
            temp_step: 0.5,
            needs_redraw: true,
            pending_config: false,
            last_config_sent: Instant::now(),
        };

        app.parameter_list_state.select(Some(0));
        Ok(app)
    }

    fn process_responses(&mut self) {
        // Process all available responses without blocking
        while let Ok(response) = self.response_rx.try_recv() {
            match response {
                WorkerResponse::Readout(readout) => {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs_f64();

                    self.temp_history.push_back(TempData {
                        timestamp: now,
                        set_temp: readout.t_set,
                        measured_temp: readout.t_measured,
                        pwm: readout.pwm,
                    });

                    if self.temp_history.len() > 1000 {
                        self.temp_history.pop_front();
                    }

                    self.check_setpoint_reached(readout.t_measured);
                    self.current_readout = Some(readout);
                    self.last_update = Instant::now();
                    self.needs_redraw = true;
                }
                WorkerResponse::Error(msg) => {
                    self.status_message = Some(format!("Error: {}", msg));
                    self.needs_redraw = true;
                }
                WorkerResponse::Status(msg) => {
                    self.status_message = Some(msg);
                    self.needs_redraw = true;
                }
            }
        }
    }

    fn check_setpoint_reached(&mut self, measured_temp: f32) {
        if let Some(ref mut setpoint_change) = self.current_setpoint_change {
            if setpoint_change.reached_time.is_none() {
                let temp_diff = (measured_temp - setpoint_change.target_temp).abs();
                if temp_diff <= self.temp_tolerance {
                    let now = Instant::now();
                    let duration = now.duration_since(setpoint_change.start_time);
                    setpoint_change.reached_time = Some(now);
                    setpoint_change.duration = Some(duration);

                    self.setpoint_history.push_back(setpoint_change.clone());
                    if self.setpoint_history.len() > 100 {
                        self.setpoint_history.pop_front();
                    }

                    self.status_message = Some(format!(
                        "Target {:.1}°C reached in {:.1}s",
                        setpoint_change.target_temp,
                        duration.as_secs_f32()
                    ));

                    self.current_setpoint_change = None;
                }
            }
        }
    }

    fn set_new_temperature(&mut self, new_temp: f32) {
        let clamped_temp = new_temp.clamp(self.current_config.t_min, self.current_config.t_max);

        if (clamped_temp - self.current_config.t_set).abs() > 0.01 {
            self.current_config.t_set = clamped_temp;

            self.current_setpoint_change = Some(SetpointChange {
                target_temp: clamped_temp,
                start_time: Instant::now(),
                reached_time: None,
                duration: None,
            });

            self.apply_configuration();
        }
    }

    fn apply_configuration(&mut self) {
        // Mark config as pending instead of sending immediately
        // Reset the timer - we want to wait from the LAST change
        self.pending_config = true;
        self.last_config_sent = Instant::now(); // Reset timer on each change
        self.needs_redraw = true;
    }

    fn send_config_if_pending(&mut self) {
        const DEBOUNCE_MS: u64 = 150; // Wait 150ms after last change before sending

        if self.pending_config
            && self.last_config_sent.elapsed() >= Duration::from_millis(DEBOUNCE_MS)
        {
            let _ = self
                .command_tx
                .send(WorkerCommand::SetConfig(self.current_config.clone()));
            self.pending_config = false;
        }
    }

    fn toggle_tec(&mut self) {
        let command = if self.tec_enabled {
            WorkerCommand::Disable
        } else {
            WorkerCommand::Enable
        };

        if self.command_tx.send(command).is_ok() {
            self.tec_enabled = !self.tec_enabled;
            self.status_message = Some(format!(
                "TEC {}",
                if self.tec_enabled {
                    "ENABLED"
                } else {
                    "DISABLED"
                }
            ));
            self.needs_redraw = true;
        }
    }

    fn increment_selected_field(&mut self) {
        let field = self.edit_field;
        let step = field.get_step(self.temp_step);
        let current = field.get_value(&self.current_config);
        let new_value = current + step;

        match field {
            EditField::TSet => self.set_new_temperature(new_value),
            EditField::P => {
                self.current_config.p = new_value;
                self.apply_configuration();
            }
            EditField::I => {
                self.current_config.i = new_value;
                self.apply_configuration();
            }
            EditField::D => {
                self.current_config.d = new_value;
                self.apply_configuration();
            }
            EditField::TMin => {
                self.current_config.t_min = new_value.max(0.0);
                self.apply_configuration();
            }
            EditField::TMax => {
                self.current_config.t_max = new_value.min(100.0);
                self.apply_configuration();
            }
        }
    }

    fn decrement_selected_field(&mut self) {
        let field = self.edit_field;
        let step = field.get_step(self.temp_step);
        let current = field.get_value(&self.current_config);
        let new_value = current - step;

        match field {
            EditField::TSet => self.set_new_temperature(new_value),
            EditField::P => {
                self.current_config.p = new_value;
                self.apply_configuration();
            }
            EditField::I => {
                self.current_config.i = new_value;
                self.apply_configuration();
            }
            EditField::D => {
                self.current_config.d = new_value;
                self.apply_configuration();
            }
            EditField::TMin => {
                self.current_config.t_min = new_value.max(0.0);
                self.apply_configuration();
            }
            EditField::TMax => {
                self.current_config.t_max = new_value.min(100.0);
                self.apply_configuration();
            }
        }
    }

    fn handle_key_input(&mut self, key: KeyCode) {
        match self.input_mode {
            InputMode::Normal => match key {
                KeyCode::Char('q') => {
                    // Handled by main loop
                }
                KeyCode::Up => {
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
                    self.edit_field = EditField::from_index(i);
                    self.needs_redraw = true;
                }
                KeyCode::Down => {
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
                    self.edit_field = EditField::from_index(i);
                    self.needs_redraw = true;
                }
                KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') => {
                    self.increment_selected_field();
                }
                KeyCode::Left | KeyCode::Char('-') | KeyCode::Char('_') => {
                    self.decrement_selected_field();
                }
                KeyCode::Char('e') | KeyCode::Enter => {
                    self.input_mode = InputMode::Editing;
                    self.edit_value.clear(); // Start with empty field
                    self.needs_redraw = true;
                }
                KeyCode::Char(' ') => {
                    self.toggle_tec();
                }
                KeyCode::Char('1') => {
                    self.temp_step = 0.1;
                    self.status_message = Some("Step: 0.1°C".to_string());
                    self.needs_redraw = true;
                }
                KeyCode::Char('2') => {
                    self.temp_step = 0.5;
                    self.status_message = Some("Step: 0.5°C".to_string());
                    self.needs_redraw = true;
                }
                KeyCode::Char('3') => {
                    self.temp_step = 1.0;
                    self.status_message = Some("Step: 1.0°C".to_string());
                    self.needs_redraw = true;
                }
                KeyCode::Char('5') => {
                    self.temp_step = 5.0;
                    self.status_message = Some("Step: 5.0°C".to_string());
                    self.needs_redraw = true;
                }
                _ => {}
            },
            InputMode::Editing => match key {
                KeyCode::Enter => {
                    if let Ok(value) = self.edit_value.parse::<f32>() {
                        let field = self.edit_field;
                        match field {
                            EditField::TSet => self.set_new_temperature(value),
                            EditField::P => {
                                self.current_config.p = value;
                                self.apply_configuration();
                            }
                            EditField::I => {
                                self.current_config.i = value;
                                self.apply_configuration();
                            }
                            EditField::D => {
                                self.current_config.d = value;
                                self.apply_configuration();
                            }
                            EditField::TMin => {
                                self.current_config.t_min = value.max(0.0);
                                self.apply_configuration();
                            }
                            EditField::TMax => {
                                self.current_config.t_max = value.min(100.0);
                                self.apply_configuration();
                            }
                        }
                    } else {
                        self.status_message = Some("Invalid value".to_string());
                    }
                    self.input_mode = InputMode::Normal;
                    self.edit_value.clear();
                    self.needs_redraw = true;
                }
                KeyCode::Esc => {
                    self.input_mode = InputMode::Normal;
                    self.edit_value.clear();
                    self.needs_redraw = true;
                }
                KeyCode::Char(c) => {
                    self.edit_value.push(c);
                    self.needs_redraw = true;
                }
                KeyCode::Backspace => {
                    self.edit_value.pop();
                    self.needs_redraw = true;
                }
                _ => {}
            },
        }
    }
}

// Worker thread that handles all serial communication
fn worker_thread(
    port_name: String,
    command_rx: Receiver<WorkerCommand>,
    response_tx: Sender<WorkerResponse>,
) {
    let mut controller = match TecController::new(&port_name) {
        Ok(ctrl) => ctrl,
        Err(e) => {
            let _ = response_tx.send(WorkerResponse::Error(format!("Failed to open port: {}", e)));
            return;
        }
    };

    let mut last_read = Instant::now();
    let read_interval = Duration::from_millis(500);

    loop {
        // Check for commands (non-blocking)
        if let Ok(command) = command_rx.try_recv() {
            match command {
                WorkerCommand::SetConfig(config) => match controller.set_configuration(&config) {
                    Ok(_) => {
                        let _ =
                            response_tx.send(WorkerResponse::Status("Config updated".to_string()));
                    }
                    Err(e) => {
                        let _ =
                            response_tx.send(WorkerResponse::Error(format!("Config error: {}", e)));
                    }
                },
                WorkerCommand::Enable => match controller.enable() {
                    Ok(_) => {
                        let _ = response_tx.send(WorkerResponse::Status("TEC ENABLED".to_string()));
                    }
                    Err(e) => {
                        let _ =
                            response_tx.send(WorkerResponse::Error(format!("Enable error: {}", e)));
                    }
                },
                WorkerCommand::Disable => match controller.disable() {
                    Ok(_) => {
                        let _ =
                            response_tx.send(WorkerResponse::Status("TEC DISABLED".to_string()));
                    }
                    Err(e) => {
                        let _ = response_tx
                            .send(WorkerResponse::Error(format!("Disable error: {}", e)));
                    }
                },
                WorkerCommand::Shutdown => {
                    break;
                }
            }
        }

        // Periodic data reading
        if last_read.elapsed() >= read_interval {
            match controller.get_single_readout() {
                Ok(readout) => {
                    let _ = response_tx.send(WorkerResponse::Readout(readout));
                }
                Err(e) => {
                    let _ = response_tx.send(WorkerResponse::Error(format!("Read error: {}", e)));
                }
            }
            last_read = Instant::now();
        }

        // Small sleep to prevent busy-waiting
        thread::sleep(Duration::from_millis(10));
    }
}

// UI rendering functions (unchanged)
fn ui(f: &mut Frame, app: &mut App) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    render_header(f, app, main_chunks[0]);

    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(35), Constraint::Min(20)])
        .split(main_chunks[1]);

    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(8)])
        .split(content_chunks[0]);

    render_current_readout(f, app, left_chunks[0]);
    render_parameters(f, app, left_chunks[1]);

    // Right side: Split into temp chart and PWM chart
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(70), // Temperature chart
            Constraint::Percentage(30), // PWM chart
        ])
        .split(content_chunks[1]);

    render_chart(f, app, right_chunks[0]);
    render_pwm_chart(f, app, right_chunks[1]);

    render_footer(f, app, main_chunks[2]);

    if app.input_mode == InputMode::Editing {
        render_edit_popup(f, app);
    }
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let tec_status = if app.tec_enabled { "ON" } else { "OFF" };
    let tec_color = if app.tec_enabled {
        Color::Green
    } else {
        Color::Red
    };

    let mut title_text = vec![
        Span::styled(
            "TEC Controller",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ Status: "),
        Span::styled(
            tec_status,
            Style::default().fg(tec_color).add_modifier(Modifier::BOLD),
        ),
    ];

    if let Some(ref readout) = app.current_readout {
        title_text.extend(vec![
            Span::raw(" │ "),
            Span::styled(
                format!("{:.1}°C", readout.t_measured),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" → "),
            Span::styled(
                format!("{:.1}°C", readout.t_set),
                Style::default().fg(Color::Magenta),
            ),
        ]);
    }

    let header = Paragraph::new(Line::from(title_text))
        .block(Block::default().borders(Borders::ALL))
        .alignment(Alignment::Center);

    f.render_widget(header, area);
}

fn render_current_readout(f: &mut Frame, app: &App, area: Rect) {
    let content = if let Some(ref readout) = app.current_readout {
        let temp_diff = readout.t_measured - readout.t_set;
        let temp_color = if temp_diff.abs() < 0.5 {
            Color::Green
        } else if temp_diff.abs() < 2.0 {
            Color::Yellow
        } else {
            Color::Red
        };

        let pwm_label = if readout.pwm >= 0.0 { "Heat" } else { "Cool" };
        let pwm_color = if readout.pwm >= 0.0 {
            Color::Red
        } else {
            Color::Blue
        };

        vec![
            Line::from(vec![
                Span::raw("Measured: "),
                Span::styled(
                    format!("{:.2}°C", readout.t_measured),
                    Style::default().fg(temp_color).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::raw("Target:   "),
                Span::styled(
                    format!("{:.2}°C", readout.t_set),
                    Style::default().fg(Color::Magenta),
                ),
            ]),
            Line::from(vec![
                Span::raw("Error:    "),
                Span::styled(
                    format!("{:+.2}°C", temp_diff),
                    Style::default().fg(temp_color),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::raw("PWM:      "),
                Span::styled(
                    format!("{:>6.1}% ", readout.pwm.abs()),
                    Style::default().fg(pwm_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(pwm_label, Style::default().fg(pwm_color)),
            ]),
            Line::from(vec![
                Span::raw("OC:       "),
                Span::styled(
                    if readout.oc {
                        "Connected"
                    } else {
                        "Disconnected"
                    },
                    Style::default().fg(if readout.oc { Color::Green } else { Color::Red }),
                ),
            ]),
        ]
    } else {
        vec![Line::from("Waiting for data...")]
    };

    let readout_widget = Paragraph::new(content).block(
        Block::default()
            .title("Current Status")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Gray)),
    );

    f.render_widget(readout_widget, area);
}

fn render_parameters(f: &mut Frame, app: &mut App, area: Rect) {
    let parameter_items: Vec<ListItem> = vec![
        EditField::TSet,
        EditField::P,
        EditField::I,
        EditField::D,
        EditField::TMin,
        EditField::TMax,
    ]
    .iter()
    .map(|field| {
        let value = field.get_value(&app.current_config);
        let step = field.get_step(app.temp_step);
        let label = field.label();

        let text = match field {
            EditField::TSet | EditField::TMin | EditField::TMax => {
                format!("{:<10} {:>7.1}°C  ±{:.1}", label, value, step)
            }
            _ => {
                format!("{:<10} {:>7.3}     ±{:.2}", label, value, step)
            }
        };

        ListItem::new(text)
    })
    .collect();

    let parameters = List::new(parameter_items)
        .block(
            Block::default()
                .title("Parameters")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(parameters, area, &mut app.parameter_list_state);
}

fn render_chart(f: &mut Frame, app: &App, area: Rect) {
    if app.temp_history.is_empty() {
        let no_data = Paragraph::new("Collecting data...")
            .block(
                Block::default()
                    .title("Temperature History")
                    .borders(Borders::ALL),
            )
            .alignment(Alignment::Center);
        f.render_widget(no_data, area);
        return;
    }

    const HISTORY_WINDOW_SECS: f64 = 120.0; // Show last 2 minutes

    let now = app.temp_history.back().unwrap().timestamp;
    let cutoff_time = now - HISTORY_WINDOW_SECS;

    // Filter to only show last 2 minutes
    let recent_data: Vec<&TempData> = app
        .temp_history
        .iter()
        .filter(|data| data.timestamp >= cutoff_time)
        .collect();

    if recent_data.is_empty() {
        let no_data = Paragraph::new("Collecting data...")
            .block(
                Block::default()
                    .title("Temperature History (Last 2 min)")
                    .borders(Borders::ALL),
            )
            .alignment(Alignment::Center);
        f.render_widget(no_data, area);
        return;
    }

    let min_time = recent_data.first().unwrap().timestamp;

    let set_data: Vec<(f64, f64)> = recent_data
        .iter()
        .map(|data| (data.timestamp - min_time, data.set_temp as f64))
        .collect();

    let measured_data: Vec<(f64, f64)> = recent_data
        .iter()
        .map(|data| (data.timestamp - min_time, data.measured_temp as f64))
        .collect();

    // Create Tmin and Tmax reference lines
    let max_time = recent_data.last().unwrap().timestamp - min_time;
    let tmin_line: Vec<(f64, f64)> = vec![
        (0.0, app.current_config.t_min as f64),
        (max_time, app.current_config.t_min as f64),
    ];
    let tmax_line: Vec<(f64, f64)> = vec![
        (0.0, app.current_config.t_max as f64),
        (max_time, app.current_config.t_max as f64),
    ];

    let datasets = vec![
        Dataset::default()
            .name("T Min")
            .marker(symbols::Marker::Dot)
            .style(Style::default().fg(Color::White))
            .graph_type(GraphType::Line)
            .data(&tmin_line),
        Dataset::default()
            .name("T Max")
            .marker(symbols::Marker::Dot)
            .style(Style::default().fg(Color::White))
            .graph_type(GraphType::Line)
            .data(&tmax_line),
        Dataset::default()
            .name("Setpoint")
            .marker(symbols::Marker::Dot)
            .style(Style::default().fg(Color::Magenta))
            .graph_type(GraphType::Line)
            .data(&set_data),
        Dataset::default()
            .name("Measured")
            .marker(symbols::Marker::Braille)
            .style(Style::default().fg(Color::Cyan))
            .graph_type(GraphType::Line)
            .data(&measured_data),
    ];

    let all_temps: Vec<f64> = recent_data
        .iter()
        .flat_map(|data| vec![data.set_temp as f64, data.measured_temp as f64])
        .collect();

    let min_temp = all_temps.iter().fold(f64::INFINITY, |a, &b| a.min(b)) - 2.0;
    let max_temp = all_temps.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b)) + 2.0;

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .title("Temperature History (Last 2 min)")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Gray)),
        )
        .x_axis(
            Axis::default()
                .title("Time (s)")
                .style(Style::default().fg(Color::Gray))
                .bounds([0.0, max_time.min(HISTORY_WINDOW_SECS)]),
        )
        .y_axis(
            Axis::default()
                .title("Temp (°C)")
                .style(Style::default().fg(Color::Gray))
                .bounds([min_temp, max_temp]),
        );

    f.render_widget(chart, area);
}

fn render_pwm_chart(f: &mut Frame, app: &App, area: Rect) {
    if app.temp_history.is_empty() {
        let no_data = Paragraph::new("Collecting data...")
            .block(
                Block::default()
                    .title("PWM Effort (Last 2 min)")
                    .borders(Borders::ALL),
            )
            .alignment(Alignment::Center);
        f.render_widget(no_data, area);
        return;
    }

    const HISTORY_WINDOW_SECS: f64 = 120.0; // Show last 2 minutes

    let now = app.temp_history.back().unwrap().timestamp;
    let cutoff_time = now - HISTORY_WINDOW_SECS;

    // Filter to only show last 2 minutes
    let recent_data: Vec<&TempData> = app
        .temp_history
        .iter()
        .filter(|data| data.timestamp >= cutoff_time)
        .collect();

    if recent_data.is_empty() {
        let no_data = Paragraph::new("Collecting data...")
            .block(
                Block::default()
                    .title("PWM Effort (Last 2 min)")
                    .borders(Borders::ALL),
            )
            .alignment(Alignment::Center);
        f.render_widget(no_data, area);
        return;
    }

    let min_time = recent_data.first().unwrap().timestamp;
    let max_time = recent_data.last().unwrap().timestamp - min_time;

    let pwm_data: Vec<(f64, f64)> = recent_data
        .iter()
        .map(|data| (data.timestamp - min_time, data.pwm as f64))
        .collect();

    let datasets = vec![
        Dataset::default()
            .name("PWM")
            .marker(symbols::Marker::Braille)
            .style(Style::default().fg(Color::Yellow))
            .graph_type(GraphType::Line)
            .data(&pwm_data),
    ];

    let all_pwm: Vec<f64> = recent_data.iter().map(|data| data.pwm as f64).collect();

    let min_pwm = all_pwm
        .iter()
        .fold(f64::INFINITY, |a, &b| a.min(b))
        .min(-5.0);
    let max_pwm = all_pwm
        .iter()
        .fold(f64::NEG_INFINITY, |a, &b| a.max(b))
        .max(5.0);

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .title("PWM Effort (Last 2 min)")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Gray)),
        )
        .x_axis(
            Axis::default()
                .title("Time (s)")
                .style(Style::default().fg(Color::Gray))
                .bounds([0.0, max_time.min(HISTORY_WINDOW_SECS)]),
        )
        .y_axis(
            Axis::default()
                .title("PWM (%)")
                .style(Style::default().fg(Color::Gray))
                .bounds([min_pwm, max_pwm]),
        );

    f.render_widget(chart, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let mut status_spans = vec![];

    if let Some(ref setpoint_change) = app.current_setpoint_change {
        let elapsed = setpoint_change.start_time.elapsed().as_secs_f32();
        status_spans.push(Span::styled(
            format!(
                "⏱ Tracking {:.1}°C for {:.1}s",
                setpoint_change.target_temp, elapsed
            ),
            Style::default().fg(Color::Cyan),
        ));
    } else if let Some(recent) = app.setpoint_history.back() {
        if let Some(duration) = recent.duration {
            status_spans.push(Span::styled(
                format!(
                    "✓ Last: {:.1}°C in {:.1}s",
                    recent.target_temp,
                    duration.as_secs_f32()
                ),
                Style::default().fg(Color::Green),
            ));
        }
    }

    if let Some(ref msg) = app.status_message {
        if !status_spans.is_empty() {
            status_spans.push(Span::raw(" │ "));
        }
        status_spans.push(Span::styled(msg, Style::default().fg(Color::Yellow)));
    }

    status_spans.push(Span::raw(" │ "));
    status_spans.push(Span::styled("↑↓", Style::default().fg(Color::Cyan)));
    status_spans.push(Span::raw(" Select  "));
    status_spans.push(Span::styled("←→/±", Style::default().fg(Color::Cyan)));
    status_spans.push(Span::raw(" Adjust  "));
    status_spans.push(Span::styled("e", Style::default().fg(Color::Cyan)));
    status_spans.push(Span::raw(" Edit  "));
    status_spans.push(Span::styled("Space", Style::default().fg(Color::Cyan)));
    status_spans.push(Span::raw(" TEC  "));
    status_spans.push(Span::styled("1-5", Style::default().fg(Color::Cyan)));
    status_spans.push(Span::raw(" Step  "));
    status_spans.push(Span::styled("q", Style::default().fg(Color::Red)));
    status_spans.push(Span::raw(" Quit"));

    let footer =
        Paragraph::new(Line::from(status_spans)).block(Block::default().borders(Borders::ALL));

    f.render_widget(footer, area);
}

fn render_edit_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(40, 20, f.area());
    f.render_widget(Clear, area);

    let edit_title = format!(
        "Edit {} (Enter to save, Esc to cancel)",
        app.edit_field.label()
    );
    let edit_popup = Paragraph::new(app.edit_value.as_str())
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .block(
            Block::default()
                .title(edit_title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );

    f.render_widget(edit_popup, area);
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

pub fn run_tui() -> Result<(), Box<dyn Error>> {
    let port_name = "/dev/serial0";

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(port_name)?;

    // Main loop - only redraw when necessary
    loop {
        // Process any responses from worker thread
        app.process_responses();

        // Send pending config if debounce period has elapsed
        app.send_config_if_pending();

        // Only redraw if needed
        if app.needs_redraw {
            terminal.draw(|f| ui(f, &mut app))?;
            app.needs_redraw = false;
        }

        // Handle input with longer timeout (reduces CPU usage)
        if crossterm::event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => {
                            let _ = app.command_tx.send(WorkerCommand::Shutdown);
                            break;
                        }
                        _ => app.handle_key_input(key.code),
                    }
                }
            }
        } else {
            // Even without events, update footer if tracking setpoint
            if app.current_setpoint_change.is_some() {
                app.needs_redraw = true;
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
