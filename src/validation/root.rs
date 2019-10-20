use super::super::FileProvider;
use super::arena::*;
use super::format::*;
use crate::parse::{ast, root};
use crate::Range;

use std::collections::{
	HashMap
};
use std::io;
use std::path::{
	Path,
	PathBuf
};

use std::error::Error;
use std::fmt::{
	Display,
	Debug,
	Formatter
};

use super::error::*;

use crate::identifier::Identifier;

use std::convert::From;

use nom::error::convert_error;

#[derive(Debug)]
pub struct Root {
	registries: HashMap<Identifier, (
		HashMap<Identifier, Index<CompoundTag>>,
		Option<Index<CompoundTag>>
	)>,

	root_modules: HashMap<String, Index<Module>>,

	compound_arena: Arena<CompoundTag>,
	enum_arena: Arena<EnumItem>,
	module_arena: Arena<Module>
}

impl Root {
	pub fn new() -> Self {
		Root {
			registries: HashMap::new(),

			root_modules: HashMap::new(),

			compound_arena: Arena::new(),
			enum_arena: Arena::new(),
			module_arena: Arena::new()
		}
	}

	pub fn get_registry(
		&self,
		name: &Identifier
	) -> Option<&(HashMap<Identifier, Index<CompoundTag>>, Option<Index<CompoundTag>>)> {
		self.registries.get(name)
	}

	pub fn get_regitry_item(&self, name: &Identifier, id: &Identifier) -> Option<&CompoundTag> {
		let (r, d) = self.registries.get(name)?;
		Some(&self.compound_arena[*r.get(id).unwrap_or(&(*d)?)])
	}

	pub fn get_compound(&self, name: Index<CompoundTag>) -> &CompoundTag {
		&self.compound_arena[name]
	}

	pub fn get_module(&self, name: Index<Module>) -> &Module {
		&self.module_arena[name]
	}

	pub fn get_enum(&self, name: Index<EnumItem>) -> &EnumItem {
		&self.enum_arena[name]
	}

	pub fn get_root_module(&self, name: &str) -> Option<Index<Module>> {
		self.root_modules.get(name).cloned()
	}

	pub fn get_modules(&self) -> std::slice::Iter<Module> {
		self.module_arena.iter()
	}

	pub fn get_compounds(&self) -> std::slice::Iter<CompoundTag> {
		self.compound_arena.iter()
	}

	pub fn get_enums(&self) -> std::slice::Iter<EnumItem> {
		self.enum_arena.iter()
	}

	/// `p` needs to be an absolute path, and must be UTF-8
	pub fn add_root_module<F, P>(
		&mut self,
		p: P,
		fp: &F
	) -> Result<(), NbtDocError> where F: FileProvider, P: AsRef<Path> {
		let module_name = p.as_ref()
			.file_stem()
			.ok_or(
				io::Error::from(io::ErrorKind::NotFound)
			)?.to_str().unwrap();
		let module_tree = ModuleTree::read(
			p.as_ref().parent().ok_or(io::Error::from(io::ErrorKind::NotFound))?,
			module_name,
			fp
		)?;
		let root = self.register_module(Module {
			children: HashMap::new(),
			parent: None
		});
		self.root_modules.insert(
			String::from(module_name),
			root
		);
		self.register_module_tree(root, &module_tree)?;
		self.preresolve_module_tree(root, &module_tree, &[module_name])?;
		self.resolve_module_tree(root, module_tree, &[module_name])?;
		Ok(())
	}

	fn register_module_tree(
		&mut self,
		rootind: Index<Module>,
		tree: &ModuleTree
	) -> Result<(), ValidationError> {
		let cast = &tree.val;
		// first register items so lower modules can resolve
		for (n, _) in cast.compounds.iter() {
			let ind = self.register_compound(CompoundTag {
				description: String::new(),
				supers: None,
				fields: HashMap::new()
			});
			self.module_arena[rootind].children.insert(n.clone(), ItemIndex::Compound(ind));
		}
		for (n, e) in cast.enums.iter() {
			let ind = self.register_enum(EnumItem {
				description: String::new(),
				et: match e.values {
					ast::EnumType::Byte(_) => EnumType::Byte(vec![]),
					ast::EnumType::Short(_) => EnumType::Short(vec![]),
					ast::EnumType::Int(_) => EnumType::Int(vec![]),
					ast::EnumType::Long(_) => EnumType::Long(vec![]),
					ast::EnumType::Float(_) => EnumType::Float(vec![]),
					ast::EnumType::Double(_) => EnumType::Double(vec![]),
					ast::EnumType::String(_) => EnumType::String(vec![]),
				}
			});
			self.module_arena[rootind].children.insert(n.clone(), ItemIndex::Enum(ind));
		}
		// register modules, which will register their items
		let mut next = Vec::with_capacity(tree.children.len());
		for (n, m) in tree.children.iter() {
			let root = self.register_module(Module {
				children: HashMap::new(),
				parent: Some(rootind)
			});
			self.module_arena[rootind].children.insert(n.clone(), ItemIndex::Module(root));
			next.push((root, m));
		}
		for (root, m) in next.into_iter() {
			self.register_module_tree(root, &m)?;
		};
		Ok(())
	}

	fn preresolve_module_tree(
		&mut self,
		rootind: Index<Module>,
		tree: &ModuleTree,
		module: &[&str]
	) -> Result<(), ValidationError> {
		let cast = &tree.val;
		let eb = |x| ValidationError::new(
			module.iter().map(|x| String::from(*x)).collect(),
			x
		);
		for (e, n) in cast.uses.iter() {
			if *e {
				let last = match n.last().ok_or(
					eb(ValidationErrorType::RootAccess)
				)? {
					ast::PathPart::Regular(v) => v,
					ast::PathPart::Super => return Err(eb(ValidationErrorType::SuperImport)),
					ast::PathPart::Root => return Err(eb(ValidationErrorType::RootAccess))
				};
				let item = self.get_item_path(
					&n,
					Some(rootind),
					&HashMap::new(),
					module
				)?;
				if let ItemIndex::Module(_) = item {
					return Err(eb(ValidationErrorType::InvalidType {
						name: match n.last().unwrap() {
							ast::PathPart::Regular(v) => v.clone(),
							_ => panic!()
						},
						ex: vec![ItemType::Compound, ItemType::Enum],
						ty: ItemType::Module
					}));
				}
				self.module_arena[rootind].children.insert(last.clone(), item);
			}
		};
		for (n, v) in &tree.children {
			let mut m = Vec::with_capacity(module.len() + 1);
			m.extend(module);
			m.push(n.as_ref());
			self.preresolve_module_tree(
				match &self.module_arena[rootind].children[n] {
					ItemIndex::Module(v) => *v,
					_ => panic!()
				},
				v,
				&m
			)?;
		}
		Ok(())
	}

	fn resolve_module_tree(
		&mut self,
		rootind: Index<Module>,
		tree: ModuleTree,
		module: &[&str]
	) -> Result<(), ValidationError> {
		let cast = tree.val;
		let eb = |x| ValidationError::new(
			module.iter().map(|x| String::from(*x)).collect(),
			x
		);
		let mut imports = HashMap::new();
		for (_, n) in cast.uses {
			imports.insert(
				match n.last().ok_or(eb(ValidationErrorType::RootAccess))? {
					ast::PathPart::Regular(s) => s.clone(),
					ast::PathPart::Root => return Err(eb(ValidationErrorType::RootAccess)),
					ast::PathPart::Super => return Err(eb(ValidationErrorType::SuperImport))
				},
				self.get_item_path(&n, Some(rootind), &HashMap::new(), module)?
			);
		}
		for (n, c) in cast.compounds {
			// this better work
			let cpdi = *match self.module_arena[rootind].children.get(&n).unwrap() {
				ItemIndex::Compound(v) => v,
				_ => panic!()
			};
			self.compound_arena[cpdi].description = c.description;
			match c.supers {
				Some(v) => self.compound_arena[cpdi].supers = Some(
					match self.get_item_path(&v, Some(rootind), &imports, module)? {
						ItemIndex::Compound(v) => v,
						v => return Err(eb(ValidationErrorType::InvalidType {
							name: n,
							ex: vec![ItemType::Compound],
							ty: match v {
								ItemIndex::Enum(_) => ItemType::Enum,
								ItemIndex::Module(_) => ItemType::Module,
								ItemIndex::Compound(_) => panic!()
							}
						}))
					}),
				None => self.compound_arena[cpdi].supers = None
			};
			for (n, t) in c.fields {
				let field = Field {
					description: t.description,
					nbttype: self.convert_field_type(t.field_type, rootind, &imports, module)?
				};
				self.compound_arena[cpdi].fields.insert(n, field);
			}
		}
		for (n, e) in cast.enums {
			let eni = *match self.module_arena[rootind].children.get(&n).unwrap() {
				ItemIndex::Enum(v) => v,
				_ => panic!()
			};
			self.enum_arena[eni].description = e.description;
			self.enum_arena[eni].et = match e.values {
				ast::EnumType::Byte(v) => EnumType::Byte(
					v.into_iter().map(|(n, v)| EnumOption {
						description: v.description,
						name: n,
						value: v.value
					}).collect()
				),
				ast::EnumType::Short(v) => EnumType::Short(
					v.into_iter().map(|(n, v)| EnumOption {
						description: v.description,
						name: n,
						value: v.value
					}).collect()
				),
				ast::EnumType::Int(v) => EnumType::Int(
					v.into_iter().map(|(n, v)| EnumOption {
						description: v.description,
						name: n,
						value: v.value
					}).collect()
				),
				ast::EnumType::Long(v) => EnumType::Long(
					v.into_iter().map(|(n, v)| EnumOption {
						description: v.description,
						name: n,
						value: v.value
					}).collect()
				),
				ast::EnumType::Float(v) => EnumType::Float(
					v.into_iter().map(|(n, v)| EnumOption {
						description: v.description,
						name: n,
						value: v.value
					}).collect()
				),
				ast::EnumType::Double(v) => EnumType::Double(
					v.into_iter().map(|(n, v)| EnumOption {
						description: v.description,
						name: n,
						value: v.value
					}).collect()
				),
				ast::EnumType::String(v) => EnumType::String(
					v.into_iter().map(|(n, v)| EnumOption {
						description: v.description,
						name: n,
						value: v.value
					}).collect()
				),
			}
		}
		for (p, d) in cast.describes {
			let target = match self.get_item_path(&p, Some(rootind), &imports, module)? {
				ItemIndex::Compound(v) => v,
				v => return Err(eb(ValidationErrorType::InvalidType {
					name: match p.last().ok_or(eb(ValidationErrorType::RootAccess))? {
						ast::PathPart::Root => String::from("root"),
						ast::PathPart::Super => String::from("super"),
						ast::PathPart::Regular(v) => v.clone()
					},
					ex: vec![ItemType::Compound],
					ty: match v {
						ItemIndex::Enum(_) => ItemType::Enum,
						ItemIndex::Module(_) => ItemType::Module,
						ItemIndex::Compound(_) => panic!()
					}
				}))
			};
			let (ref mut dt, ref mut def) = {
				if !self.registries.contains_key(&d.describe_type) {
					self.registries.insert(d.describe_type.clone(), (HashMap::new(), None));
				};
				self.registries.get_mut(&d.describe_type).unwrap()
			};
			if let Some(targets) = d.targets {
				for n in targets {
					if dt.contains_key(&n) {
						return Err(eb(ValidationErrorType::DuplicateDescribe {
							reg: d.describe_type,
							t: Some(n)
						}))
					}
					dt.insert(n, target);
				}
			} else {
				if def.is_some() {
					return Err(eb(
						ValidationErrorType::DuplicateDescribe {
							reg: d.describe_type,
							t: None
						}
					))
				}
				*def = Some(target);
			}
		};
		for (n, v) in tree.children {
			let mut m = Vec::with_capacity(module.len() + 1);
			m.extend(module);
			m.push(n.as_ref());
			self.resolve_module_tree(
				match self.module_arena[rootind].children[&n] {
					ItemIndex::Module(v) => v,
					_ => panic!()
				},
				v,
				&m
			)?;
		}
		Ok(())
	}

	fn get_item_path(
		&self,
		path: &[ast::PathPart],
		rel: Option<Index<Module>>,
		imports: &HashMap<String, ItemIndex>,
		module: &[&str]
	) -> Result<ItemIndex, ValidationError> {
		let eb = |x| ValidationError::new(
			module.iter().map(|x| String::from(*x)).collect(),
			x
		);
		if path.is_empty() {
			return Err(eb(ValidationErrorType::RootAccess))
		}
		let mut start = true;
		let mut current = rel;
		for part in &path[0..path.len() - 1] {
			current = match self.get_child(
				part,
				current,
				if start {
					start = false;
					Some(imports)
				} else {
					None
				},
				module
			)? {
				None => None,
				Some(v) => match v {
					ItemIndex::Module(m) => Some(m),
					v => return Err(eb(ValidationErrorType::InvalidType {
						name: match part {
							ast::PathPart::Regular(v) => v.clone(),
							// Super **must** lead to a module, and Root has already been covered
							_ => panic!()
						},
						ex: vec![ItemType::Module],
						ty: match v {
							ItemIndex::Compound(_) => ItemType::Compound,
							ItemIndex::Enum(_) => ItemType::Enum,
							ItemIndex::Module(_) => panic!()
						}
					}))
				}
			}
		};
		self.get_child(path.last().unwrap(), current, if start {
			Some(imports)
		} else {
			None
		}, module)?.ok_or(eb(ValidationErrorType::RootAccess))
	}

	fn get_child(
		&self,
		part: &ast::PathPart,
		path: Option<Index<Module>>,
		imports: Option<&HashMap<String, ItemIndex>>,
		module: &[&str]
	) -> Result<Option<ItemIndex>, ValidationError> {
		let eb = |x| ValidationError::new(
			module.iter().map(|x| String::from(*x)).collect(),
			x
		);
		Ok(match part {
			ast::PathPart::Root => None,
			ast::PathPart::Super => self.module_arena[
				path.ok_or(eb(ValidationErrorType::RootAccess))?
			].parent.map(ItemIndex::Module),
			ast::PathPart::Regular(v) => Some(match path {
				Some(i) => self.module_arena[i].children.get(v.as_str()).cloned().or_else(
						|| imports.and_then(|h| h.get(v.as_str())).cloned()
					),
				None => self.root_modules.get(v.as_str()).map(|v| ItemIndex::Module(*v))
			}.ok_or_else(|| eb(ValidationErrorType::UnresolvedName(v.clone())))?)
		})
	}

	fn register_module(&mut self, module: Module) -> Index<Module> {
		self.module_arena.push(module)
	}

	fn register_compound(&mut self, item: CompoundTag) -> Index<CompoundTag> {
		self.compound_arena.push(item)
	}

	fn register_enum(&mut self, item: EnumItem) -> Index<EnumItem> {
		self.enum_arena.push(item)
	}

	fn convert_field_type(
		&self,
		ft: ast::FieldType,
		root: Index<Module>,
		imports: &HashMap<String, ItemIndex>,
		module: &[&str]
	) -> Result<NbtValue, ValidationError> {
		let eb = |x| ValidationError::new(
			module.iter().map(|x| String::from(*x)).collect(),
			x
		);
		Ok(match ft {
			ast::FieldType::BooleanType => NbtValue::Boolean,
			ast::FieldType::StringType => NbtValue::String,
			ast::FieldType::NamedType(v) => {
				let item = self.get_item_path(&v, Some(root), imports, module)?;
				match item {
					ItemIndex::Module(_) => return Err(eb(ValidationErrorType::InvalidType {
						name: match v.last().unwrap() {
							ast::PathPart::Regular(v) => v.clone(),
							ast::PathPart::Root => String::from("root"),
							ast::PathPart::Super => String::from("super")
						},
						ex: vec![ItemType::Compound, ItemType::Enum],
						ty: ItemType::Module
					})),
					ItemIndex::Compound(v) => NbtValue::Compound(v),
					ItemIndex::Enum(v) => NbtValue::Enum(v)
				}
			},
			ast::FieldType::ArrayType(v) => match v {
				ast::NumberArrayType::Byte { value_range, len_range } => 
					NbtValue::ByteArray(NumberArrayTag {
						length_range: len_range.map(|x|convert_range(x, 0, i32::max_value())),
						value_range: value_range.map(|x|convert_range(
							x,
							i8::min_value(),
							i8::max_value()
						))
					}),
				ast::NumberArrayType::Int { value_range, len_range } => 
					NbtValue::IntArray(NumberArrayTag {
						length_range: len_range.map(|x|convert_range(x, 0, i32::max_value())),
						value_range: value_range.map(|x|convert_range(
							x,
							i32::min_value(),
							i32::max_value()
						))
					}),
				ast::NumberArrayType::Long { value_range, len_range } => 
					NbtValue::LongArray(NumberArrayTag {
						length_range: len_range.map(|x|convert_range(x, 0, i32::max_value())),
						value_range: value_range.map(|x|convert_range(
							x,
							i64::min_value(),
							i64::max_value()
						))
					})
			},
			ast::FieldType::NumberType(v) => match v {
				ast::NumberPrimitiveType::Byte(range) => NbtValue::Byte(NumberTag {
					range: range.map(|x|convert_range(x, i8::min_value(), i8::max_value()))
				}),
				ast::NumberPrimitiveType::Short(range) => NbtValue::Short(NumberTag {
					range: range.map(|x|convert_range(x, i16::min_value(), i16::max_value()))
				}),
				ast::NumberPrimitiveType::Int(range) => NbtValue::Int(NumberTag {
					range: range.map(|x|convert_range(x, i32::min_value(), i32::max_value()))
				}),
				ast::NumberPrimitiveType::Long(range) => NbtValue::Long(NumberTag {
					range: range.map(|x|convert_range(x, i64::min_value(), i64::max_value()))
				}),
				ast::NumberPrimitiveType::Float(range) => NbtValue::Float(NumberTag {
					range: range.map(|x|convert_range(x, std::f32::NEG_INFINITY,  std::f32::INFINITY))
				}),
				ast::NumberPrimitiveType::Double(range) => NbtValue::Double(NumberTag {
					range: range.map(|x|convert_range(x, std::f64::NEG_INFINITY,  std::f64::INFINITY))
				})
			},
			ast::FieldType::ListType { item_type, len_range } => NbtValue::List {
				length_range: len_range.map(|x|convert_range(x, 0, i32::max_value())),
				value_type: Box::from(
					self.convert_field_type(*item_type, root, imports, module)?
				)
			},
			ast::FieldType::IndexType { target, path } => NbtValue::Index { path, target },
			ast::FieldType::IdType(v) => NbtValue::Id(v),
			ast::FieldType::OrType(v) => NbtValue::Or(v.into_iter().map(
				|x| self.convert_field_type(x, root, imports, module)
			).collect::<Result<Vec<NbtValue>, ValidationError>>()?)
		})
	}
}

fn convert_range<T: Copy>(range: ast::Range<T>, min: T, max: T) -> Range<T> {
	match range {
		ast::Range::Single(v) => Range(v, v),
		ast::Range::Both(l, h) => Range(l, h),
		ast::Range::Low(l) => Range(l, max),
		ast::Range::High(h) => Range(min, h)
	}
}

struct ModuleTree {
	val: ast::NbtDocFile,
	children: HashMap<String, ModuleTree>
}

impl ModuleTree {
	pub fn read<'a, P, F>(
		dir: P,
		name: &str,
		fp: &F
	) -> Result<Self, NbtDocError> where P: AsRef<Path>, F: FileProvider {
		let filename = format!("{}.nbtdoc", name);
		let mut newdir = PathBuf::from(dir.as_ref());
		let file = if fp.exists(dir.as_ref().join(&filename)) {
			fp.read_file(dir.as_ref().join(&filename))?
		} else {
			newdir.push(name);
			fp.read_file(dir.as_ref().join(name).join("mod.nbtdoc"))?
		};
		let mut out = ModuleTree {
			val: match root::<nom::error::VerboseError<&str>>(&file) {
				Ok(v) => v,
				Err(e) => return Err(match e {
					nom::Err::Error(e) | nom::Err::Failure(e) =>
						NbtDocError::Parse(convert_error(&file, e)),
					nom::Err::Incomplete(e) => NbtDocError::Parse(match e {
						nom::Needed::Size(e) => format!("Needs {} more bytes of data", e),
						nom::Needed::Unknown => format!("Needs unknown number of bytes")
					})
				})
			}.1,
			children: HashMap::new()
		};
		for x in out.val.mods.iter() {
			out.children.insert(x.clone(), ModuleTree::read(&newdir, x, fp)?);
		}
		Ok(out)
	}
}

#[derive(Debug)]
pub enum NbtDocError {
	Io(io::Error),
	Parse(String),
	Validation(ValidationError)
}

impl Display for NbtDocError {
	fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
		match self {
			Self::Io(e) => write!(f, "{}", e),
			Self::Parse(e) => write!(f, "{}", e),
			Self::Validation(e) => write!(f, "{}", e)
		}
	}
}

impl From<io::Error> for NbtDocError {
	fn from(e: io::Error) -> Self {
		Self::Io(e)
	}
}

impl From<ValidationError> for NbtDocError {
	fn from(e: ValidationError) -> Self {
		Self::Validation(e)
	}
}

impl Error for NbtDocError {}

#[cfg(test)]
mod tests {

	use super::*;

	struct MockFileProvider {
		map: HashMap<PathBuf, &'static str>
	}

	impl FileProvider for MockFileProvider {
		fn read_file<F: AsRef<Path>>(&self, f: F) -> io::Result<String> {
			Ok(String::from(*self.map.get(f.as_ref()).unwrap()))
		}

		fn exists<F: AsRef<Path>>(&self, f: F) -> bool {
			self.map.contains_key(&PathBuf::from(f.as_ref()))
		}
	}

	#[test]
	fn small_files() -> Result<(), NbtDocError> {
		let mut fp = MockFileProvider {
			map: HashMap::new()
		};
		fp.map.insert(
			PathBuf::from("/small_file_root.nbtdoc"),
			include_str!("../../tests/small_file_root.nbtdoc")
		);
		fp.map.insert(
			PathBuf::from("/small_file_sibling.nbtdoc"),
			include_str!("../../tests/small_file_sibling.nbtdoc")
		);
		let mut root = Root::new();
		root.add_root_module("/small_file_root.nbtdoc", &fp)?;
		Ok(())
	}
}