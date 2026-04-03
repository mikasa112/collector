use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Expr, Field, Fields, Ident, ItemStruct, Lit, LitFloat, LitInt, LitStr, Result, Token, Type,
    parse::Parse, parse::ParseStream, parse_macro_input, punctuated::Punctuated,
};

#[proc_macro_attribute]
pub fn modbus_config(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new_spanned(
            proc_macro2::TokenStream::from(attr),
            "modbus_config does not accept attribute arguments",
        )
        .to_compile_error()
        .into();
    }

    let input = parse_macro_input!(item as ItemStruct);
    match expand_modbus_config(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn modbus_meter(attr: TokenStream, item: TokenStream) -> TokenStream {
    modbus_config(attr, item)
}

fn expand_modbus_config(mut input: ItemStruct) -> Result<proc_macro2::TokenStream> {
    let struct_ident = input.ident.clone();
    let configs_ident = Ident::new(
        &format!("{}_MODBUS_CONFIGS", struct_ident.to_string().to_uppercase()),
        struct_ident.span(),
    );
    let vis = input.vis.clone();

    let Fields::Named(fields) = &mut input.fields else {
        return Err(syn::Error::new_spanned(
            &input,
            "modbus_config requires a struct with named fields",
        ));
    };

    let mut configs = Vec::with_capacity(fields.named.len());
    for field in fields.named.iter_mut() {
        configs.push(parse_modbus_field(field)?);
    }

    let count = configs.len();
    let generated = configs.iter().map(FieldConfig::to_tokens);

    Ok(quote! {
        #input

        #vis const #configs_ident: [::collector_core::config::modbus_conf::ModbusConfig; #count] = [
            #(#generated),*
        ];

        impl #struct_ident {
            #vis fn modbus_configs() -> &'static [::collector_core::config::modbus_conf::ModbusConfig] {
                &#configs_ident
            }
        }
    })
}

fn parse_modbus_field(field: &mut Field) -> Result<FieldConfig> {
    let field_ident = field
        .ident
        .clone()
        .ok_or_else(|| syn::Error::new_spanned(&*field, "modbus_config requires named fields"))?;

    let mut modbus_attr = None;
    field.attrs.retain(|attr| {
        let keep = !attr.path().is_ident("modbus");
        if !keep {
            modbus_attr = Some(attr.clone());
        }
        keep
    });

    let attr = modbus_attr.ok_or_else(|| {
        syn::Error::new_spanned(&field_ident, "missing #[modbus(...)] attribute on field")
    })?;
    let args = attr.parse_args_with(Punctuated::<ModbusArg, Token![,]>::parse_terminated)?;

    let mut config = FieldConfig::new(field_ident.to_string(), field.ty.clone());
    for arg in args {
        config.apply(arg, &field_ident)?;
    }
    config.finish(&field_ident)?;
    Ok(config)
}

#[derive(Clone)]
struct FieldConfig {
    field_name: String,
    ty: Type,
    id: Option<u16>,
    name: Option<String>,
    data_type: Option<Ident>,
    unit: Option<String>,
    remarks: Option<String>,
    register_address: Option<u16>,
    register_type: Option<Ident>,
    quantity: Option<u16>,
    byte_order: Option<Ident>,
    scale: Option<f64>,
    offset: Option<f64>,
}

impl FieldConfig {
    fn new(field_name: String, ty: Type) -> Self {
        Self {
            field_name,
            ty,
            id: None,
            name: None,
            data_type: None,
            unit: None,
            remarks: None,
            register_address: None,
            register_type: None,
            quantity: None,
            byte_order: None,
            scale: None,
            offset: None,
        }
    }

    fn apply(&mut self, arg: ModbusArg, field_ident: &Ident) -> Result<()> {
        match arg {
            ModbusArg::Int { key, value } if key == "id" => self.id = Some(parse_u16(value, &key)?),
            ModbusArg::String { key, value } if key == "name" => self.name = Some(value.value()),
            ModbusArg::Ident { key, value } if key == "data_type" => self.data_type = Some(value),
            ModbusArg::String { key, value } if key == "unit" => self.unit = Some(value.value()),
            ModbusArg::String { key, value } if key == "remarks" => {
                self.remarks = Some(value.value())
            }
            ModbusArg::Int { key, value } if key == "register_address" => {
                self.register_address = Some(parse_u16(value, &key)?)
            }
            ModbusArg::Ident { key, value } if key == "register_type" => {
                self.register_type = Some(value)
            }
            ModbusArg::Int { key, value } if key == "quantity" => {
                self.quantity = Some(parse_u16(value, &key)?)
            }
            ModbusArg::Ident { key, value } if key == "byte_order" => self.byte_order = Some(value),
            ModbusArg::Float { key, value } if key == "scale" => {
                self.scale = Some(value.base10_parse()?)
            }
            ModbusArg::Int { key, value } if key == "scale" => {
                self.scale = Some(value.base10_parse::<f64>()?)
            }
            ModbusArg::Float { key, value } if key == "offset" => {
                self.offset = Some(value.base10_parse()?)
            }
            ModbusArg::Int { key, value } if key == "offset" => {
                self.offset = Some(value.base10_parse::<f64>()?)
            }
            other => {
                return Err(syn::Error::new(
                    other.value_span(),
                    format!(
                        "unsupported modbus attribute `{}` on field `{}`",
                        other.key(),
                        field_ident
                    ),
                ));
            }
        }
        Ok(())
    }

    fn finish(&mut self, field_ident: &Ident) -> Result<()> {
        if self.name.is_none() {
            self.name = Some(self.field_name.clone());
        }
        if self.data_type.is_none() {
            self.data_type = Some(infer_data_type(&self.ty).ok_or_else(|| {
                syn::Error::new_spanned(
                    field_ident,
                    "missing data_type and failed to infer it from the field type",
                )
            })?);
        }
        if self.quantity.is_none() {
            let data_type = self.data_type.as_ref().expect("set above");
            self.quantity = Some(default_quantity(data_type));
        }
        if self.scale.is_none() {
            self.scale = Some(1.0);
        }
        if self.offset.is_none() {
            self.offset = Some(0.0);
        }

        let required = [
            ("id", self.id.is_some()),
            ("register_address", self.register_address.is_some()),
            ("register_type", self.register_type.is_some()),
        ];
        for (name, exists) in required {
            if !exists {
                return Err(syn::Error::new_spanned(
                    field_ident,
                    format!("missing required modbus attribute `{name}`"),
                ));
            }
        }
        Ok(())
    }

    fn to_tokens(&self) -> proc_macro2::TokenStream {
        let id = self.id.expect("validated");
        let name = LitStr::new(
            self.name.as_deref().expect("validated"),
            proc_macro2::Span::call_site(),
        );
        let data_type = self.data_type.as_ref().expect("validated");
        let register_address = self.register_address.expect("validated");
        let register_type = self.register_type.as_ref().expect("validated");
        let quantity = self.quantity.expect("validated");
        let scale = self.scale.expect("validated");
        let offset = self.offset.expect("validated");

        let unit = self
            .unit
            .as_ref()
            .map(|value| {
                let lit = LitStr::new(value, proc_macro2::Span::call_site());
                quote!(Some(#lit))
            })
            .unwrap_or_else(|| quote!(None));
        let remarks = self
            .remarks
            .as_ref()
            .map(|value| {
                let lit = LitStr::new(value, proc_macro2::Span::call_site());
                quote!(Some(#lit))
            })
            .unwrap_or_else(|| quote!(None));
        let byte_order = self
            .byte_order
            .as_ref()
            .map(|value| quote!(Some(::collector_core::config::modbus_conf::ByteOrder::#value)))
            .unwrap_or_else(|| quote!(None));

        quote! {
            ::collector_core::config::modbus_conf::ModbusConfig {
                id: #id,
                name: #name,
                data_type: ::collector_core::config::modbus_conf::ModbusDataType::#data_type,
                unit: #unit,
                remarks: #remarks,
                register_address: #register_address,
                register_type: ::collector_core::config::modbus_conf::RegisterType::#register_type,
                quantity: #quantity,
                byte_order: #byte_order,
                scale: #scale,
                offset: #offset,
            }
        }
    }
}

fn infer_data_type(ty: &Type) -> Option<Ident> {
    match ty {
        Type::Path(path) => path.path.segments.last().and_then(|segment| {
            let name = segment.ident.to_string();
            match name.as_str() {
                "bool" => Some(Ident::new("Bool", segment.ident.span())),
                "u16" => Some(Ident::new("U16", segment.ident.span())),
                "i16" => Some(Ident::new("I16", segment.ident.span())),
                "u32" => Some(Ident::new("U32", segment.ident.span())),
                "i32" => Some(Ident::new("I32", segment.ident.span())),
                _ => None,
            }
        }),
        _ => None,
    }
}

fn default_quantity(data_type: &Ident) -> u16 {
    match data_type.to_string().as_str() {
        "U32" | "I32" => 2,
        _ => 1,
    }
}

fn parse_u16(value: LitInt, key: &str) -> Result<u16> {
    let parsed = value.base10_parse::<u32>()?;
    if parsed > u16::MAX as u32 {
        return Err(syn::Error::new_spanned(
            value,
            format!("{key} exceeds u16 range"),
        ));
    }
    Ok(parsed as u16)
}

enum ModbusArg {
    Int { key: String, value: LitInt },
    Float { key: String, value: LitFloat },
    String { key: String, value: LitStr },
    Ident { key: String, value: Ident },
}

impl ModbusArg {
    fn key(&self) -> &str {
        match self {
            ModbusArg::Int { key, .. }
            | ModbusArg::Float { key, .. }
            | ModbusArg::String { key, .. }
            | ModbusArg::Ident { key, .. } => key,
        }
    }

    fn value_span(&self) -> proc_macro2::Span {
        match self {
            ModbusArg::Int { value, .. } => value.span(),
            ModbusArg::Float { value, .. } => value.span(),
            ModbusArg::String { value, .. } => value.span(),
            ModbusArg::Ident { value, .. } => value.span(),
        }
    }
}

impl Parse for ModbusArg {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let key = input.parse::<Ident>()?;
        input.parse::<Token![=]>()?;
        let key_str = key.to_string();
        let expr = input.parse::<Expr>()?;

        match expr {
            Expr::Lit(expr_lit) => match expr_lit.lit {
                Lit::Int(value) => Ok(ModbusArg::Int {
                    key: key_str,
                    value,
                }),
                Lit::Float(value) => Ok(ModbusArg::Float {
                    key: key_str,
                    value,
                }),
                Lit::Str(value) => Ok(ModbusArg::String {
                    key: key_str,
                    value,
                }),
                _ => Err(syn::Error::new_spanned(
                    expr_lit,
                    "unsupported literal in modbus attribute",
                )),
            },
            Expr::Path(expr_path) if expr_path.path.segments.len() == 1 => {
                let value = expr_path.path.segments[0].ident.clone();
                Ok(ModbusArg::Ident {
                    key: key_str,
                    value,
                })
            }
            other => Err(syn::Error::new_spanned(
                other,
                "unsupported value in modbus attribute",
            )),
        }
    }
}
