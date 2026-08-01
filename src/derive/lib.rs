// Part of the Unified Mechanism for Acquisition of Measured Intensity
// (UMAMI), see README and LICENSE files for more info.

//! Derive macros for inputs, outputs, and post-processing steps.

use syn::*;
use quote::{ToTokens, format_ident};
use quip::quip;
use darling::{FromDeriveInput, FromField, ast};

#[derive(FromDeriveInput)]
#[darling(attributes(param, params), supports(struct_named))]
struct Info {
    // name of the type
    ident: Ident,
    // possible generics to reproduce
    generics: Generics,
    data: ast::Data<(), Param>,
    // module kind/type reported via the synthetic `_info` entry, e.g.
    // `#[params(kind = "input", type = "mesy")]` -- required on every
    // `#[derive(HasParams)]` struct.
    kind: String,
    #[darling(rename = "type")]
    type_name: String,
}

#[derive(FromField, Debug)]
#[darling(attributes(param))]
struct Param {
    // syn
    ident: Option<Ident>,
    ty: Type,
    // ours
    name: Option<String>,
    datatype: Option<String>,
    help: Option<String>,
    has_setter: Option<bool>,
    readonly: Option<bool>,
    runtime_only: Option<bool>,
}

#[proc_macro_derive(HasParams, attributes(param, params))]
pub fn derive_has_params(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let info = Info::from_derive_input(&input).unwrap();

    let mut getters = vec![];
    let mut setters = vec![];

    let kind = &info.kind;
    let type_name = &info.type_name;
    getters.push(quip! {
        if full {
            params.insert("_info".to_string(), crate::params::info_entry(#kind, #type_name));
        }
    });

    info.data.map_struct_fields(|param| {
        if param.help.is_none() {
            return;
        }
        let id = param.ident.unwrap();
        let name = param.name.unwrap_or(id.to_string());
        let typ = param.datatype.unwrap_or(param.ty.to_token_stream().to_string());
        let help = param.help.unwrap_or(String::new());

        let readonly = param.readonly.unwrap_or(false);
        let runtime_only = param.runtime_only.unwrap_or(false);
        getters.push(quip! {
            let value = serde_json::to_value(&self.#id)
                .context(#{format!("Parameter {name} cannot be serialized to JSON, \
                                    this is a programming bug")})?;
            params.insert(#name.to_string(), if full {
                serde_json::to_value(crate::params::ParamInfo {
                    datatype: #typ.into(),
                    help: #help.into(),
                    readonly: #readonly,
                    runtime_only: #runtime_only,
                    value,
                }).context("Serializing parameter value")?
            } else {
                serde_json::json!({"value": value})
            });
        });

        if param.readonly.unwrap_or(false) {
            setters.push(quip! {
                if params.contains_key(#name) {
                    Err(anyhow::anyhow!(
                        #{format!("Parameter {name} is read-only")}))?;
                }
            });
            return;
        }

        let msg = format!("Value {{:?}} is invalid for parameter {name}, \
                           needs to be: {typ}");
        let setting = if param.has_setter.unwrap_or(false) {
            let set_ident = format_ident!("set_{}", id);
            quip! {
                self.#set_ident(parsed).context(#{format!("Error setting parameter {name}")})?;
            }
        } else {
            quip! { self.#id = parsed; }
        };
        setters.push(quip! {
            if let Some(value) = params.remove(#name) {
                crate::lprintln!(INFO, [name]
                                 #{format!("Setting parameter {name} to {{value}}")});
                let errmsg = format!(#msg, value);
                let parsed = serde_json::from_value(value).context(errmsg)?;
                #setting
            }
        });
    });

    let type_name_const = {
        let (imp, ty, wher) = info.generics.split_for_impl();
        quip! {
            impl #imp #{info.ident} #ty #wher {
                pub const TYPE_NAME: &'static str = #type_name;
            }
        }
    };

    let (imp, ty, wher) = info.generics.split_for_impl();
    let result = quip! {
        #type_name_const

        impl #imp crate::params::HasParams for #{info.ident} #ty #wher {
            fn get_params(&self, full: bool) -> crate::error::UResult<crate::params::ParamMap> {
                use anyhow::Context;
                let mut params = crate::params::ParamMap::new();
                #(#getters)*
                Ok(params)
            }

            fn update_params(&mut self, name: crate::command::ModuleId,
                             mut params: crate::params::ParamMap) -> crate::error::UResult<()> {
                use anyhow::Context;
                #(#setters)*
                Ok(())
            }
        }
    };
    // println!("{}", result);
    result.into()
}
