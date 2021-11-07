#![feature(box_patterns)]

use proc_macro::TokenStream;
use quote::format_ident;
use quote::quote;
use std::env;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use syn::parse_macro_input;

use syn::ItemFn;

mod attrs;
mod derive_fn;
mod derive_struct;
mod docs;
mod meta;

use crate::derive_fn::process_function;
use crate::derive_struct::process_struct;
use crate::meta::Glue;
use crate::meta::Type;

const METAFILE: &str = "bindings.json";

#[cfg(target_endian = "little")]
const ENDIANNESS: bool = true;

#[cfg(target_endian = "big")]
const ENDIANNESS: bool = false;

#[proc_macro_attribute]
pub fn deno_bindgen(attr: TokenStream, input: TokenStream) -> TokenStream {
  let mut metadata: Glue = match OpenOptions::new().read(true).open(METAFILE) {
    Ok(mut fd) => {
      let mut meta = String::new();
      fd.read_to_string(&mut meta)
        .expect("Error reading meta file");

      serde_json::from_str(&meta).unwrap()
    }
    Err(_) => Glue {
      little_endian: ENDIANNESS,
      name: env::var("CARGO_CRATE_NAME").unwrap_or_default(),
      ..Default::default()
    },
  };

  let mut metafile = OpenOptions::new()
    .write(true)
    .create(true)
    .open(METAFILE)
    .expect("Error opening meta file");

  match syn::parse::<ItemFn>(input.clone()) {
    Ok(func) => {
      let attr = parse_macro_input!(attr as syn::AttributeArgs);
      let symbol = process_function(func.clone(), attr, &mut metadata).unwrap();

      let mut params = vec![];
      let mut overrides = vec![];
      let mut input_idents = vec![];
      let mut c_index = 0;

      for parameter in symbol.parameters {
        match parameter {
          Type::StructEnum { .. } => {
            let ident = format_ident!("arg{}", c_index.to_string());
            params.push(quote! { #ident: *const u8 });

            c_index += 1;
            let len_ident = format_ident!("arg{}", c_index.to_string());
            params.push(quote! { #len_ident: usize });

            overrides.push(quote! {
              let buf = unsafe {
                ::std::slice::from_raw_parts(#ident, #len_ident)
              };
              let #ident = deno_bindgen::serde_json::from_slice(buf).unwrap();
            });

            input_idents.push(ident);
          }
          Type::Str | Type::Buffer | Type::BufferMut => {
            let ident = format_ident!("arg{}", c_index.to_string());
            match parameter {
              Type::Str | Type::Buffer => {
                params.push(quote! { #ident: *const u8 })
              }
              Type::BufferMut => params.push(quote! { #ident: *mut u8 }),
              _ => unreachable!(),
            };

            c_index += 1;
            let len_ident = format_ident!("arg{}", c_index.to_string());
            params.push(quote! { #len_ident: usize });

            let return_type = match parameter {
              Type::Str => quote! { ::std::str::from_utf8(buf).unwrap() },
              Type::Buffer | Type::BufferMut => quote! { buf },
              _ => unreachable!(),
            };

            let buf_expr = match parameter {
              Type::Str | Type::Buffer => {
                quote! { let buf = ::std::slice::from_raw_parts(#ident, #len_ident); }
              }
              Type::BufferMut => {
                quote! { let mut buf = ::std::slice::from_raw_parts_mut(#ident, #len_ident);
                }
              }
              _ => unreachable!(),
            };

            overrides.push(quote! {
              let #ident = unsafe {
                #buf_expr
                #return_type
              };
            });

            input_idents.push(ident);
          }
          // TODO
          _ => {
            let ident = format_ident!("arg{}", c_index.to_string());
            let ty = syn::Type::from(parameter);
            params.push(quote! { #ident: #ty });
            input_idents.push(ident);
          }
        };

        c_index += 1;
      }

      let name = &func.sig.ident;
      let fn_inputs = &func.sig.inputs;
      let fn_output = &func.sig.output;
      let fn_block = &func.block;

      let overrides = overrides
        .iter()
        .fold(quote! {}, |acc, new| quote! { #acc #new });

      metafile
        .write_all(&serde_json::to_vec(&metadata).unwrap())
        .unwrap();

      TokenStream::from(quote! {
        #[no_mangle]
        pub extern "C" fn #name (#(#params,) *) #fn_output {
          fn __inner_impl (#fn_inputs) #fn_output #fn_block
          #overrides
          __inner_impl(#(#input_idents, ) *)
        }
      })
    }
    Err(_) => {
      let input = syn::parse_macro_input!(input as syn::DeriveInput);
      process_struct(&mut metadata, input.clone()).unwrap();

      metafile
        .write_all(&serde_json::to_vec(&metadata).unwrap())
        .unwrap();

      TokenStream::from(quote! {
        #[derive(::serde::Deserialize)]
        #input
      })
    }
  }
}