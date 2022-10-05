// TODO just throwing everything into mod.rs for now, don't want to deal with keeping things clean
// yet
use crate::core::{
    clean::ast as ca,
    compile::{builtin as bi, namespace::*},
    util::*,
};
// LOL I JUST LEARNED THAT I COULD DO THIS INSTEAD OF IMPORTING FROM CRATE
use super::{
    builtin::{Builtin, BuiltinSource},
    check::{DefinedType, ParamType, Ty, TyName},
};
use std::collections::HashMap;

enum Error {
    InvalidBase(Ty),
    EnumAccount,
    InvalidEnumVariant,
    InvalidClassField,
}

impl Error {
    fn core(self, loc: &Location) -> CoreError {
        match self {
            Self::InvalidBase(ty) => CoreError::make_raw(
                format!("cannot inherit from \"{:?}\"", ty),
                "Help: inheritance in this version of Seahorse is limited to a few builtin types (Account and Enum). This will change in a future release."
            ),
            Self::EnumAccount => CoreError::make_raw("accounts may not be enums", ""),
            Self::InvalidEnumVariant => CoreError::make_raw(
                "invalid enum variant",
                "Help: `Enum` is a special type in Seahorse - you may only define variants like this:\n\n    variant_name = <unique int>"
            ),
            Self::InvalidClassField => CoreError::make_raw(
                "invalid class field",
                "Help: make sure your field has nothing but a type annotation:\n\n    field_name: Type"
            ),
        }
        .located(loc.clone())
    }
}

/// The output of the "sign" step. Adds additional information to namespaces to make typechecking
/// easier - each defined object gets a signature that is used to determine its type and the types
/// of associated operations on the object (e.g. indexing into a struct or calling a method).
#[derive(Clone, Debug)]
pub struct SignOutput {
    pub namespace_output: NamespaceOutput,
    pub tree: Tree<Signed>,
}

/// Signatures associated with every object defined in a namespace.
pub type Signed = HashMap<String, Signature>;

/// Signature of an object.
#[derive(Clone, Debug)]
pub enum Signature {
    Class(ClassSignature),
    Function(FunctionSignature),
    Builtin(Builtin),
}

/// Signature for a Python `class`.
#[derive(Clone, Debug)]
pub enum ClassSignature {
    Struct(StructSignature),
    Enum(EnumSignature),
}

/// Signature for a class that gets treated as a struct.
#[derive(Clone, Debug)]
pub struct StructSignature {
    pub is_account: bool,
    pub bases: Vec<Ty>,
    pub fields: HashMap<String, Ty>,
    // methods: HashMap<String, Ty>,
    // TODO special methods like constructor?
}

/// Signature for a class that gets treated as an enum.
#[derive(Clone, Debug)]
pub struct EnumSignature {
    pub variants: HashMap<String, ()>,
}

/// Signature for a function.
#[derive(Clone, Debug)]
pub struct FunctionSignature {
    pub params: Vec<(String, Ty, ParamType)>,
    pub returns: Ty,
}

impl Tree<Signed> {
    /// Correct typenames.
    fn correct(&self, ty: Ty) -> Ty {
        match ty {
            Ty::Generic(name, params) => {
                let name = match name {
                    TyName::Defined(path, _) => {
                        let signature = self.get_leaf_ext(&path);

                        let defined = match signature.unwrap() {
                            Signature::Class(ClassSignature::Struct(StructSignature {
                                is_account: true,
                                ..
                            })) => DefinedType::Account,
                            Signature::Class(ClassSignature::Enum(..)) => DefinedType::Enum,
                            _ => DefinedType::Struct,
                        };

                        TyName::Defined(path, defined)
                    }
                    name => name,
                };

                Ty::Generic(
                    name,
                    params.into_iter().map(|ty| self.correct(ty)).collect(),
                )
            }
            ty => ty,
        }
    }
}

impl TryFrom<NamespaceOutput> for SignOutput {
    type Error = CoreError;

    fn try_from(namespace_output: NamespaceOutput) -> CResult<Self> {
        // Runs in two passes:
        // 1. collects most of the signature info, but naively puts every non-builtin type under
        //    `TyName::Defined`.
        // 2. corrects the type names to include `TyName::DefinedAccount` as well.

        let raw_tree = namespace_output
            .tree
            .clone()
            .map_with_path(|namespace, abs| {
                let mut signatures = HashMap::new();

                for (name, export) in namespace.iter() {
                    match export {
                        Export::Item(Item::Defined(def)) => {
                            let signature = build_signature(def, abs, &namespace_output.tree)?;
                            signatures.insert(name.clone(), signature);
                        }
                        Export::Item(Item::Builtin(builtin)) => {
                            signatures.insert(builtin.name(), Signature::Builtin(builtin.clone()));
                        }
                        _ => {}
                    }
                }

                Ok(signatures)
            })
            .transpose()?;

        let tree = raw_tree.clone().map(|signatures| {
            HashMap::from_iter(signatures.into_iter().map(|(name, signature)| {
                (
                    name,
                    match signature {
                        Signature::Class(ClassSignature::Struct(StructSignature {
                            is_account,
                            bases,
                            fields,
                        })) => Signature::Class(ClassSignature::Struct(StructSignature {
                            is_account,
                            bases,
                            fields: fields
                                .into_iter()
                                .map(|(name, ty)| (name, raw_tree.correct(ty)))
                                .collect(),
                        })),
                        Signature::Function(FunctionSignature { params, returns }) => {
                            Signature::Function(FunctionSignature {
                                params: params
                                    .into_iter()
                                    .map(|(name, ty, is_required)| {
                                        (name, raw_tree.correct(ty), is_required)
                                    })
                                    .collect(),
                                returns: raw_tree.correct(returns),
                            })
                        }
                        signature => signature,
                    },
                )
            }))
        });

        Ok(SignOutput {
            namespace_output,
            tree,
        })
    }
}

fn build_signature(
    def: &ca::TopLevelStatement,
    // TODO: fairly common pattern to pass around an absolute path to a leaf within a tree + its
    // root, might be cleaner to turn that into a context struct and make the build_ functions
    // member functions
    abs: &Vec<String>,
    root: &Tree<Namespace>,
) -> CResult<Signature> {
    let Located(loc, obj) = def;

    match obj {
        ca::TopLevelStatementObj::ClassDef { body, bases, .. } => {
            let mut is_account = false;
            let mut is_enum = false;
            let mut bases_ = vec![];
            for base in bases.iter() {
                let base = root.build_ty(base, abs)?;
                match base {
                    Ty::Generic(
                        TyName::Builtin(bi::Builtin::Prelude(bi::prelude::Prelude::Account)),
                        _,
                    ) => {
                        bases_.push(base.clone());
                        is_account = true;
                    }
                    Ty::Generic(
                        TyName::Builtin(bi::Builtin::Prelude(bi::prelude::Prelude::Enum)),
                        _,
                    ) => {
                        is_enum = true;
                    }
                    ty => {
                        return Err(Error::InvalidBase(ty).core(&loc));
                    }
                }
            }

            if is_account && is_enum {
                return Err(Error::EnumAccount.core(loc));
            }

            if is_enum {
                let mut variants = HashMap::new();
                for statement in body.iter() {
                    let Located(loc, obj) = statement;

                    match obj {
                        ca::ClassDefStatementObj::FieldDef { name, ty, value } => {
                            if ty.is_some() || value.is_none() {
                                return Err(Error::InvalidEnumVariant.core(loc));
                            }

                            variants.insert(name.clone(), ());
                        }
                    }
                }

                Ok(Signature::Class(ClassSignature::Enum(EnumSignature {
                    variants,
                })))
            } else {
                let mut fields = HashMap::new();
                for statement in body.iter() {
                    let Located(loc, obj) = statement;

                    match obj {
                        ca::ClassDefStatementObj::FieldDef { name, ty, value } => {
                            if value.is_some() || ty.is_none() {
                                return Err(Error::InvalidClassField.core(loc));
                            }

                            fields.insert(name.clone(), root.build_ty(&ty.as_ref().unwrap(), abs)?);
                        }
                    }
                }

                Ok(Signature::Class(ClassSignature::Struct(StructSignature {
                    is_account,
                    fields,
                    bases: bases_,
                })))
            }
        }
        ca::TopLevelStatementObj::FunctionDef(ca::FunctionDef {
            params,
            // decorator_list,
            returns,
            ..
        }) => {
            let params = params
                .params
                .iter()
                .map(|Located(_, ca::ParamObj { arg, annotation })| {
                    let ty = root.build_ty(annotation, abs)?;
                    Ok((arg.clone(), ty, ParamType::Required))
                })
                .collect::<CResult<Vec<_>>>()?;

            let returns = returns
                .as_ref()
                .map(|ty| root.build_ty(ty, abs))
                .unwrap_or(Ok(Ty::Generic(
                    TyName::Builtin(bi::python::Python::None.into()),
                    vec![],
                )))?;

            Ok(Signature::Function(FunctionSignature { params, returns }))
        }
        _ => panic!(),
    }
    .map_err(|err: Error| err.core(loc))
}

pub fn sign(registry: NamespaceOutput) -> Result<SignOutput, CoreError> {
    registry.try_into()
}