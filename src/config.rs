use std::{fmt, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use smithay_client_toolkit::shell::wlr_layer::{Anchor, Layer};
use toml::{Table, Value};

const DEFAULT: &str = include_str!("default.toml");

const FILE_NAME: &str = "hlactivate.toml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub lang: Lang,
    pub layer: LayerName,
    pub outputs: Vec<String>,
    pub anchor: Vec<Edge>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub margin: [i32; 4],
    pub font: String,
    pub color: Color,
    pub lines: Vec<Line>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Line {
    #[serde(default)]
    pub text: String,
    pub size: f32,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Eng,
    Ru,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::Eng => "en",
            Lang::Ru => "ru",
        }
    }

    fn texts(self) -> &'static [&'static str] {
        match self {
            Lang::Eng => &["Activate Linux", "Go to Settings to activate Linux."],
            Lang::Ru => &[
                "Активация Linux",
                "Чтобы активировать Linux, перейдите в раздел «Параметры».",
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerName {
    Background,
    Bottom,
    Top,
    Overlay,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(try_from = "String")]
pub struct Color(pub cosmic_text::Color);

impl fmt::Debug for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b, a] = self.0.as_rgba();
        write!(f, "#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

impl Config {
    pub fn layer(&self) -> Layer {
        match self.layer {
            LayerName::Background => Layer::Background,
            LayerName::Bottom => Layer::Bottom,
            LayerName::Top => Layer::Top,
            LayerName::Overlay => Layer::Overlay,
        }
    }

    pub fn anchor(&self) -> Anchor {
        self.anchor.iter().fold(Anchor::empty(), |acc, edge| {
            acc | match edge {
                Edge::Top => Anchor::TOP,
                Edge::Bottom => Anchor::BOTTOM,
                Edge::Left => Anchor::LEFT,
                Edge::Right => Anchor::RIGHT,
            }
        })
    }

    pub fn wants_output(&self, name: Option<&str>) -> bool {
        self.outputs.is_empty() || name.is_some_and(|n| self.outputs.iter().any(|o| o == n))
    }
}

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let err = || format!("неверный цвет {s:?}, ожидается \"#RRGGBB\" или \"#RRGGBBAA\"");
        let hex = s.strip_prefix('#').ok_or_else(err)?;
        if !matches!(hex.len(), 6 | 8) {
            return Err(err());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| err());
        let alpha = if hex.len() == 8 { byte(6)? } else { 0xFF };
        Ok(Color(cosmic_text::Color::rgba(
            byte(0)?,
            byte(2)?,
            byte(4)?,
            alpha,
        )))
    }
}

pub fn path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(dir.join(FILE_NAME))
}

pub fn load() -> Result<Config> {
    let mut table: Table = toml::from_str(DEFAULT).expect("вшитый default.toml некорректен");

    if let Some(path) = path() {
        if path.exists() {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("не удалось прочитать {}", path.display()))?;
            let user: Table = toml::from_str(&text)
                .with_context(|| format!("ошибка синтаксиса в {}", path.display()))?;
            merge(&mut table, user);
            println!("конфиг: {}", path.display());
        } else {
            println!("{} нет, использую встроенный конфиг", path.display());
        }
    }

    let mut config: Config = table.try_into().context("ошибка в конфиге")?;
    let texts = config.lang.texts();
    for (i, line) in config.lines.iter_mut().enumerate() {
        if line.text.is_empty() {
            let Some(text) = texts.get(i) else {
                bail!("у строки №{} нет text, а перевода для неё нет", i + 1);
            };
            line.text = text.to_string();
        }
    }
    if config.width == Some(0) || config.height == Some(0) {
        bail!("width и height должны быть больше нуля");
    }
    println!("{config:#?}");
    Ok(config)
}

fn merge(base: &mut Table, over: Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(base_inner)), Value::Table(over_inner)) => {
                merge(base_inner, over_inner)
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}
