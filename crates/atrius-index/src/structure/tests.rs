//! The structural reading on fixtures in Rust, TypeScript, Python and Go. Every assertion is on the
//! symbol list, never on a snapshot, so a test names the fact it checks.

use crate::outline::{self, Definition};

use super::*;

/// The definitions of a fixture, read through the bundled manifest for its extension.
fn read(path: &str, text: &str) -> Vec<Definition> {
    outline::read(path, text, outline::CHUNK_BUDGET).definitions
}

/// The one definition with this name and kind.
fn find<'a>(defs: &'a [Definition], name: &str, kind: Kind) -> &'a Definition {
    defs.iter()
        .find(|d| d.name == name && d.symbol_kind == kind)
        .unwrap_or_else(|| panic!("no {} {name} in {:#?}", kind.name(), names(defs)))
}

/// Every definition as `kind name in container`, for a failure message.
fn names(defs: &[Definition]) -> Vec<String> {
    defs.iter().map(|d| format!("{} {} in {:?}", d.symbol_kind.name(), d.name, d.container)).collect()
}

const RUST: &str = r#"use std::collections::HashMap;
use crate::layout::{Layout, Line as L, caret::*};

/// A painter for one frame.
pub struct Painter {
    pub width: f32,
    lines: Vec<Line>,
    pub(crate) scale: f32,
}

pub enum Mode {
    Insert,
    Replace(char),
    Visual { start: usize },
}

impl Painter {
    /// Makes a painter.
    pub fn new(width: f32) -> Self {
        let layout = Layout::new(width);
        let count: usize = 3;
        Painter { width, lines: Vec::new(), scale: 1.0 }
    }

    fn draw(&mut self, ui: &mut Ui, at: Pos2) -> Response {
        todo!()
    }
}

impl Drop for Painter {
    fn drop(&mut self) {}
}

mod inner {
    pub fn helper() -> u8 { 1 }
}

pub type Lines = Vec<Line>;
"#;

#[test]
fn a_rust_method_belongs_to_its_impl_and_carries_its_parameters_and_return() {
    let defs = read("src/painter.rs", RUST);
    let new = find(&defs, "new", Kind::Method);
    assert_eq!(new.container.as_deref(), Some("Painter"));
    assert_eq!(new.visibility, Visibility::Public);
    assert_eq!(new.doc.as_deref(), Some("Makes a painter."));
    assert_eq!(new.returns.as_deref(), Some("Self"));
    assert_eq!(new.parameter_text(), "width: f32");
    let draw = find(&defs, "draw", Kind::Method);
    assert_eq!(draw.visibility, Visibility::Private);
    assert_eq!(draw.parameter_text(), "self, ui: &mut Ui, at: Pos2");
    assert_eq!(draw.returns.as_deref(), Some("Response"));
    let drop = find(&defs, "drop", Kind::Method);
    assert_eq!(drop.container.as_deref(), Some("Painter"), "impl Trait for Type names Type");
    let helper = find(&defs, "helper", Kind::Function);
    assert_eq!(helper.container.as_deref(), Some("inner"), "a function in a module is not a method");
    assert_eq!(helper.returns.as_deref(), Some("u8"));
}

#[test]
fn a_rust_struct_lists_its_fields_with_their_types_and_an_enum_its_variants() {
    let defs = read("src/painter.rs", RUST);
    let painter = find(&defs, "Painter", Kind::Struct);
    assert_eq!(painter.doc.as_deref(), Some("A painter for one frame."));
    let width = find(&defs, "width", Kind::Field);
    assert_eq!(width.container.as_deref(), Some("Painter"));
    assert_eq!(width.type_text.as_deref(), Some("f32"));
    assert_eq!(width.visibility, Visibility::Public);
    let lines = find(&defs, "lines", Kind::Field);
    assert_eq!(lines.type_text.as_deref(), Some("Vec<Line>"));
    assert_eq!(lines.visibility, Visibility::Private);
    assert_eq!(find(&defs, "scale", Kind::Field).visibility, Visibility::Crate);
    for variant in ["Insert", "Replace", "Visual"] {
        assert_eq!(find(&defs, variant, Kind::Variant).container.as_deref(), Some("Mode"));
    }
    assert!(!defs.iter().any(|d| d.name == "start"), "a variant's own fields are not the enum's");
    assert_eq!(find(&defs, "Lines", Kind::TypeAlias).type_text.as_deref(), Some("Vec<Line>"));
}

#[test]
fn a_rust_variable_takes_its_type_from_an_annotation_or_a_constructor() {
    let defs = read("src/painter.rs", RUST);
    assert_eq!(find(&defs, "layout", Kind::Variable).type_text.as_deref(), Some("Layout"));
    assert_eq!(find(&defs, "count", Kind::Variable).type_text.as_deref(), Some("usize"));
}

#[test]
fn rust_imports_are_taken_apart_into_paths() {
    let outline = outline::read("src/painter.rs", RUST, outline::CHUNK_BUDGET);
    let paths: Vec<(String, Option<String>, bool)> =
        outline.imports.iter().map(|i| (i.path.join("::"), i.alias.clone(), i.glob)).collect();
    assert_eq!(
        paths,
        vec![
            ("std::collections::HashMap".to_owned(), None, false),
            ("crate::layout::Layout".to_owned(), None, false),
            ("crate::layout::Line".to_owned(), Some("L".to_owned()), false),
            ("crate::layout::caret".to_owned(), None, true),
        ]
    );
}

const TYPESCRIPT: &str = r#"import React, { useState, type FC as Component } from 'react';
import * as path from 'node:path';
import './side-effect.css';

/** What a card shows. */
export interface CardProps {
  title: string;
  count?: number;
  onPick(id: string): void;
}

export class Board {
  private cards: CardProps[] = [];
  protected name = 'board';

  render(scale: number): string {
    return '';
  }
}

export enum Lane {
  Todo,
  Done = 2,
}

export type Size = {
  width: number;
  height: number;
};

export const add = (a: number, b: number): number => a + b;

function helper(x: string) {
  const board = new Board();
}
"#;

#[test]
fn a_typescript_interface_lists_its_properties_and_methods() {
    let defs = read("src/card.ts", TYPESCRIPT);
    let props = find(&defs, "CardProps", Kind::Interface);
    assert_eq!(props.doc.as_deref(), Some("What a card shows."));
    assert_eq!(props.visibility, Visibility::Public);
    let title = find(&defs, "title", Kind::Field);
    assert_eq!(title.container.as_deref(), Some("CardProps"));
    assert_eq!(title.type_text.as_deref(), Some("string"));
    assert_eq!(find(&defs, "count", Kind::Field).type_text.as_deref(), Some("number"));
    let pick = find(&defs, "onPick", Kind::Method);
    assert_eq!(pick.parameter_text(), "id: string");
    assert_eq!(pick.returns.as_deref(), Some("void"));
}

#[test]
fn a_typescript_class_has_methods_and_fields_with_visibility() {
    let defs = read("src/card.ts", TYPESCRIPT);
    let render = find(&defs, "render", Kind::Method);
    assert_eq!(render.container.as_deref(), Some("Board"));
    assert_eq!(render.visibility, Visibility::Public);
    assert_eq!(render.returns.as_deref(), Some("string"));
    let cards = find(&defs, "cards", Kind::Field);
    assert_eq!(cards.visibility, Visibility::Private);
    assert_eq!(cards.type_text.as_deref(), Some("CardProps[]"));
    assert_eq!(find(&defs, "name", Kind::Field).visibility, Visibility::Crate);
    assert_eq!(find(&defs, "Todo", Kind::Variant).container.as_deref(), Some("Lane"));
    assert_eq!(find(&defs, "Done", Kind::Variant).container.as_deref(), Some("Lane"));
}

#[test]
fn an_arrow_function_constant_is_a_function_and_a_new_names_its_type() {
    let defs = read("src/card.ts", TYPESCRIPT);
    let add = find(&defs, "add", Kind::Function);
    assert_eq!(add.parameter_text(), "a: number, b: number");
    assert_eq!(add.returns.as_deref(), Some("number"));
    assert_eq!(find(&defs, "board", Kind::Variable).type_text.as_deref(), Some("Board"));
    assert_eq!(find(&defs, "helper", Kind::Function).visibility, Visibility::Private);
    let size = find(&defs, "Size", Kind::TypeAlias);
    assert_eq!(size.visibility, Visibility::Public);
    assert_eq!(find(&defs, "width", Kind::Field).container.as_deref(), Some("Size"));
}

#[test]
fn typescript_imports_keep_their_specifier_first() {
    let outline = outline::read("src/card.ts", TYPESCRIPT, outline::CHUNK_BUDGET);
    let paths: Vec<(Vec<String>, Option<String>, bool)> =
        outline.imports.iter().map(|i| (i.path.clone(), i.alias.clone(), i.glob)).collect();
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<String>>();
    assert_eq!(
        paths,
        vec![
            (s(&["react", "useState"]), None, false),
            (s(&["react", "FC"]), Some("Component".to_owned()), false),
            (s(&["react", "default"]), Some("React".to_owned()), false),
            (s(&["node:path"]), Some("path".to_owned()), true),
            (s(&["./side-effect.css"]), None, false),
        ]
    );
}

#[test]
fn a_half_typed_file_still_reads_what_is_there() {
    let text = "pub struct Half {\n    pub a: u8,\n    pub b: \nimpl Half {\n    pub fn go(&self, x: ";
    let defs = read("src/half.rs", text);
    assert!(defs.iter().any(|d| d.name == "Half"));
    assert!(defs.iter().any(|d| d.name == "go"), "{:#?}", names(&defs));
}

#[test]
fn python_methods_and_go_receivers_find_their_container() {
    let python = "class Session:\n    timeout: int = 3\n\n    def send(self, request: Request) -> Response:\n        \"\"\"Sends one request.\"\"\"\n        pass\n";
    let defs = read("requests/sessions.py", python);
    let send = find(&defs, "send", Kind::Method);
    assert_eq!(send.container.as_deref(), Some("Session"));
    assert_eq!(send.returns.as_deref(), Some("Response"));
    assert_eq!(send.doc.as_deref(), Some("Sends one request."));
    assert_eq!(find(&defs, "timeout", Kind::Field).type_text.as_deref(), Some("int"));
    let go = "package cobra\n\ntype Command struct {\n\tUse string\n\tShort string\n}\n\nfunc (c *Command) Execute(args []string) error {\n\treturn nil\n}\n";
    let defs = read("command.go", go);
    let execute = find(&defs, "Execute", Kind::Method);
    assert_eq!(execute.container.as_deref(), Some("Command"));
    assert_eq!(execute.returns.as_deref(), Some("error"));
    assert_eq!(execute.parameter_text(), "args: []string");
    assert_eq!(find(&defs, "Use", Kind::Field).type_text.as_deref(), Some("string"));
}
