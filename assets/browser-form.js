async (element, action, requested) => {
  if (!element || !element.isConnected) return {ok:false,error:'stale ref; take a fresh browser text snapshot'};
  if (!element.getClientRects().length || getComputedStyle(element).visibility === 'hidden') return {ok:false,error:'element is hidden'};
  if (element.matches(':disabled,[aria-disabled="true"]')) return {ok:false,error:'element is disabled'};
  if (element.readOnly || element.getAttribute('aria-readonly') === 'true') return {ok:false,error:'element is readonly'};
  const tag = element.tagName.toLowerCase();
  const type = element.type || tag;
  const input = () => element.dispatchEvent(new InputEvent('input', {bubbles:true,inputType:'insertText',data:typeof requested === 'string' ? requested : null}));
  const change = () => element.dispatchEvent(new Event('change', {bubbles:true}));
  let read;
  element.focus();
  if (action === 'fill') {
    if (element.isContentEditable) {
      element.textContent = requested;
      read = () => element.textContent;
    } else {
      if (!['input','textarea'].includes(tag) || ['checkbox','radio','file','button','submit','reset','image','hidden'].includes(type)) return {ok:false,error:`fill does not support ${type}; use check, upload, or click_ref`};
      const proto = tag === 'textarea' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto,'value').set.call(element, requested);
      read = () => element.value;
      if (read() !== requested) return {ok:false,error:`${type} rejected the requested value`,value:read()};
    }
    input(); change();
  } else if (action === 'select') {
    if (tag !== 'select') return {ok:false,error:'select requires a native select element'};
    if (!element.multiple && requested.length !== 1) return {ok:false,error:'single select requires exactly one value'};
    const options = [...element.options];
    if (requested.some(value => !options.some(option => option.value === value && !option.disabled && !option.closest('optgroup[disabled]')))) return {ok:false,error:'requested option is missing or disabled'};
    options.forEach(option => { option.selected = requested.includes(option.value); });
    read = () => [...element.selectedOptions].map(option => option.value).sort();
    requested = [...new Set(requested)].sort();
    input(); change();
  } else if (action === 'check') {
    if (tag !== 'input' || !['checkbox','radio'].includes(type)) return {ok:false,error:'check requires a checkbox or radio input'};
    if (type === 'radio' && !requested) return {ok:false,error:'choose another radio option to clear this one'};
    if (element.checked !== requested) element.click();
    read = () => element.checked;
  } else return {ok:false,error:'unknown form action'};
  // Let controlled inputs reconcile before claiming the value stuck.
  await new Promise(resolve => setTimeout(resolve, 50));
  const value = read();
  if (!element.isConnected || JSON.stringify(value) !== JSON.stringify(requested)) return {ok:false,error:'page did not retain the requested value; inspect a fresh snapshot',value};
  return {ok:true,action,type,value,valid:element.validity ? element.validity.valid : true,validation_message:element.validationMessage || ''};
}
